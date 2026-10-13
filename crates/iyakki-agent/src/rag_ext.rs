//! External RAG backends: query a vector database or retrieval service you already run, and merge
//! its results with the local knowledge base.
//!
//! * **Qdrant** (REST) and **Chroma** (HTTP API v2) take a query *vector*, so the embedding model
//!   configured in Settings must match the one the collection was built with.
//! * **HTTP** is a generic contract for custom retrieval services:
//!   `POST url {"query": "...", "k": 5, "vector": [...]?}` →
//!   `{"results": [{"text": "...", "source": "...", "score": 0.9}]}`.
//!
//! Credentials are secret references (see [`crate::vault`]), resolved per request.

use std::sync::Arc;

use iyakki_proxy::Upstream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::kb::Hit;
use crate::vault::{LocalVault, Secrets};

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

fn default_tenant() -> String {
    "default_tenant".into()
}
fn default_database() -> String {
    "default_database".into()
}

fn seg_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && s != "." && s != ".."
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

    fn url(&self) -> &str {
        match self {
            ExternalRag::Qdrant { url, .. } | ExternalRag::Chroma { url, .. } | ExternalRag::Http { url, .. } => url,
        }
    }

    fn key_ref(&self) -> Option<&String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{HashEmbed, TextEmbedder};
    use iyakki_proxy::UpstreamResponse;
    use std::sync::Mutex;

    struct Mock {
        calls: Mutex<Vec<(String, String, Vec<(String, String)>, String)>>,
        reply: Box<dyn Fn(&str, &str) -> (u16, String) + Send + Sync>,
    }
    impl Upstream for Mock {
        fn request(&self, m: &str, url: &str, h: &[(String, String)], b: &[u8]) -> Result<UpstreamResponse, String> {
            self.calls.lock().unwrap().push((m.into(), url.into(), h.to_vec(), String::from_utf8_lossy(b).into()));
            let (status, body) = (self.reply)(m, url);
            Ok(UpstreamResponse { status, body: body.into_bytes() })
        }
    }
    fn client(cfg: ExternalRag, reply: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> (ExtClient, Arc<Mock>) {
        std::env::set_var("IYAKKI_TEST_EXT_KEY", "EXTKEY");
        let m = Arc::new(Mock { calls: Default::default(), reply: Box::new(reply) });
        (ExtClient::new(cfg, Secrets::new(None, m.clone()), m.clone()), m)
    }

    #[test]
    fn qdrant_search_shape() {
        let cfg = ExternalRag::Qdrant { id: "q".into(), url: "http://qdrant:6333/".into(), collection: "docs".into(), key_ref: Some("env:IYAKKI_TEST_EXT_KEY".into()) };
        let (c, m) = client(cfg, |_, _| (200, r#"{"result":[{"id":1,"score":0.91,"payload":{"text":"vault holds keys","source":"ops.md"}},{"id":2,"score":0.5,"payload":{"nothing":"useful"}}]}"#.into()));
        let h = c.search("keys", Some(&[0.1, 0.2]), 3).unwrap();
        assert_eq!(h.len(), 1, "points without text are skipped");
        assert_eq!((h[0].text.as_str(), h[0].source.as_str(), h[0].ns.as_str()), ("vault holds keys", "ops.md", "ext:q"));
        let call = m.calls.lock().unwrap()[0].clone();
        assert_eq!(call.1, "http://qdrant:6333/collections/docs/points/search");
        assert!(call.2.contains(&("api-key".into(), "EXTKEY".into())));
        let body: Value = serde_json::from_str(&call.3).unwrap();
        assert_eq!((body["limit"].clone(), body["with_payload"].clone()), (json!(3), json!(true)));
        assert!(c.search("x", None, 3).unwrap_err().contains("embedding"));
    }

    #[test]
    fn chroma_resolves_collection_id_then_queries() {
        let cfg = ExternalRag::Chroma { id: "c".into(), url: "http://chroma:8000".into(), tenant: "default_tenant".into(), database: "default_database".into(), collection: "docs".into(), token_ref: None };
        let (c, m) = client(cfg, |meth, url| match (meth, url) {
            ("GET", u) if u.ends_with("/collections/docs") => (200, r#"{"id":"b1d2-3c4","name":"docs"}"#.into()),
            (_, u) if u.ends_with("/collections/b1d2-3c4/query") => (200, r#"{"ids":[["a","b"]],"documents":[["first doc","second doc"]],"metadatas":[[{"source":"s1"},null]],"distances":[[0.0,1.0]]}"#.into()),
            _ => (404, "{}".into()),
        });
        let h = c.search("q", Some(&[1.0]), 2).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].source.as_str(), h[0].score), ("s1", 1.0));
        assert_eq!(h[1].score, 0.5, "distance 1.0 maps to 0.5");
        assert!(m.calls.lock().unwrap()[1].1.starts_with("http://chroma:8000/api/v2/tenants/default_tenant/databases/default_database/collections/b1d2-3c4/query"));
    }

    #[test]
    fn generic_http_contract() {
        let cfg = ExternalRag::Http { id: "corp".into(), url: "https://search.corp/api/rag".into(), key_ref: Some("env:IYAKKI_TEST_EXT_KEY".into()) };
        let (c, m) = client(cfg.clone(), |_, _| (200, r#"{"results":[{"text":"a","source":"x","score":0.9},{"text":"b","score":0.1}]}"#.into()));
        let h = c.search("question", None, 1).unwrap();
        assert_eq!((h.len(), h[0].text.as_str(), h[0].score), (1, "a", 0.9), "capped at k");
        let call = m.calls.lock().unwrap()[0].clone();
        assert!(call.2.contains(&("Authorization".into(), "Bearer EXTKEY".into())));
        assert_eq!(serde_json::from_str::<Value>(&call.3).unwrap()["query"], "question");
        // a bare array is accepted too; garbage is an error
        let (c2, _) = client(cfg.clone(), |_, _| (200, r#"[{"content":"z"}]"#.into()));
        assert_eq!(c2.search("q", None, 5).unwrap()[0].text, "z");
        let (c3, _) = client(cfg, |_, _| (200, r#"{"unexpected":1}"#.into()));
        assert!(c3.search("q", None, 5).is_err());
    }

    #[test]
    fn errors_and_validation() {
        let cfg = ExternalRag::Qdrant { id: "q".into(), url: "http://x".into(), collection: "docs".into(), key_ref: None };
        let (c, _) = client(cfg.clone(), |_, _| (401, "Must provide an API key".into()));
        assert_eq!(c.search("q", Some(&[1.0]), 1).unwrap_err(), "HTTP 401: Must provide an API key");
        assert!(cfg.validate().is_ok());
        let mut bad = cfg.clone();
        if let ExternalRag::Qdrant { collection, .. } = &mut bad {
            *collection = "a/../b".into();
        }
        assert!(bad.validate().is_err(), "collection names go into URLs and must be safe");
        let raw_key = ExternalRag::Http { id: "h".into(), url: "http://x".into(), key_ref: Some("sk-pasted-raw".into()) };
        assert!(raw_key.validate().unwrap_err().contains("must start with"));
        assert!(ExternalRag::Http { id: "H".into(), url: "http://x".into(), key_ref: None }.validate().is_err());
        assert!(ExternalRag::Http { id: "h".into(), url: "ftp://x".into(), key_ref: None }.validate().is_err());
        let (c, _) = client(ExternalRag::Qdrant { id: "q".into(), url: "http://x".into(), collection: "docs".into(), key_ref: Some("vault:missing".into()) }, |_, _| (200, "{}".into()));
        assert!(c.search("q", Some(&[1.0]), 1).unwrap_err().contains("credential unavailable"));
    }

    // ---- live checks against real servers (skipped unless the env vars are set)

    fn live_upstream() -> Arc<dyn Upstream> {
        Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(std::time::Duration::from_secs(20)))
    }

    #[test]
    fn live_qdrant() {
        let Ok(url) = std::env::var("IYAKKI_LIVE_QDRANT") else { return };
        let up = live_upstream();
        let e = HashEmbed::default();
        let docs = [("ops.md", "The deploy key is stored in the vault under prod"), ("food.md", "Lunch options are sandwich soup and salad"), ("net.md", "mDNS announces services on the local network")];
        let vecs = e.embed(&docs.iter().map(|d| d.1.to_string()).collect::<Vec<_>>()).unwrap();
        let h = vec![("Content-Type".to_string(), "application/json".to_string())];
        let _ = up.request("DELETE", &format!("{url}/collections/iyakki_live"), &h, b"");
        let r = up.request("PUT", &format!("{url}/collections/iyakki_live"), &h, json!({"vectors": {"size": 256, "distance": "Cosine"}}).to_string().as_bytes()).unwrap();
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let pts: Vec<Value> = docs.iter().zip(&vecs).enumerate().map(|(i, (d, v))| json!({"id": i + 1, "vector": v, "payload": {"text": d.1, "source": d.0}})).collect();
        let r = up.request("PUT", &format!("{url}/collections/iyakki_live/points?wait=true"), &h, json!({"points": pts}).to_string().as_bytes()).unwrap();
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let c = ExtClient::new(ExternalRag::Qdrant { id: "q".into(), url: url.clone(), collection: "iyakki_live".into(), key_ref: None }, Secrets::new(None, up.clone()), up);
        println!("qdrant test: {}", c.test().unwrap());
        let qv = e.embed(&["where is the deploy key".to_string()]).unwrap().remove(0);
        let hits = c.search("where is the deploy key", Some(&qv), 2).unwrap();
        println!("qdrant top hit: {:?}", hits.first().map(|h| (&h.source, h.score)));
        assert_eq!(hits[0].source, "ops.md");
    }

    #[test]
    fn live_chroma() {
        let Ok(url) = std::env::var("IYAKKI_LIVE_CHROMA") else { return };
        let up = live_upstream();
        let e = HashEmbed::default();
        let docs = [("ops.md", "The deploy key is stored in the vault under prod"), ("food.md", "Lunch options are sandwich soup and salad"), ("net.md", "mDNS announces services on the local network")];
        let vecs = e.embed(&docs.iter().map(|d| d.1.to_string()).collect::<Vec<_>>()).unwrap();
        let h = vec![("Content-Type".to_string(), "application/json".to_string())];
        let base = format!("{url}/api/v2/tenants/default_tenant/databases/default_database/collections");
        let _ = up.request("DELETE", &format!("{base}/iyakki_live"), &h, b"");
        let r = up.request("POST", &base, &h, json!({"name": "iyakki_live", "get_or_create": true}).to_string().as_bytes()).unwrap();
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let id = serde_json::from_slice::<Value>(&r.body).unwrap()["id"].as_str().unwrap().to_string();
        let add = json!({"ids": ["1", "2", "3"], "embeddings": vecs, "documents": docs.iter().map(|d| d.1).collect::<Vec<_>>(), "metadatas": docs.iter().map(|d| json!({"source": d.0})).collect::<Vec<_>>()});
        let r = up.request("POST", &format!("{base}/{id}/add"), &h, add.to_string().as_bytes()).unwrap();
        assert!((200..300).contains(&r.status), "{} {}", r.status, String::from_utf8_lossy(&r.body));
        let c = ExtClient::new(ExternalRag::Chroma { id: "c".into(), url, tenant: "default_tenant".into(), database: "default_database".into(), collection: "iyakki_live".into(), token_ref: None }, Secrets::new(None, up.clone()), up);
        println!("chroma test: {}", c.test().unwrap());
        let qv = e.embed(&["where is the deploy key".to_string()]).unwrap().remove(0);
        let hits = c.search("where is the deploy key", Some(&qv), 2).unwrap();
        println!("chroma top hit: {:?}", hits.first().map(|h| (&h.source, h.score)));
        assert_eq!(hits[0].source, "ops.md");
    }
}
