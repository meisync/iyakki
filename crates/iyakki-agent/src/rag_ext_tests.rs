use iyakki_shared::rag_ext::*;
use iyakki_proxy::Upstream;
use serde_json::{json, Value};
use std::sync::Arc;
use iyakki_shared::vault::Secrets;
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
