//! Model-based entity extraction for Graph RAG (F-RAG-18). OFF by default, behind two things the person controls:
//! * the setting (`<data>/rag-entities.json`, a model name or nothing; set by `rag_graph_entities_set`, which always asks), and
//! * a card EVERY time the extraction runs (`rag_graph_entities_build`, a hard ask that no session grant covers): it names the model
//!   and the server and says how many passages and characters of which collection are sent.
//!
//! The model is a chat model of the person's own Lemonade server on this computer (never a cloud provider: nothing leaves the
//! machine). Each passage is sent once, with a fixed instruction; the answer is read as a JSON list of names, every name is cleaned
//! and limited, and nothing the model says is run or trusted as more than a name. The result is kept beside the graph
//! (`rag-entities-NS.cache.json`, 0600) with the size of the collection it was made from, and is added to the graph only while the
//! setting names a model and the collection is the same size.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_proxy::Upstream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::kb::KnowledgeBase;

/// Passages sent in one run, at most, and characters of one passage sent, at most.
pub const MAX_PASSAGES: usize = 400;
pub const MAX_PASSAGE_CHARS: usize = 1500;
pub const MAX_PER_PASSAGE: usize = 24;

#[derive(Serialize, Deserialize, Default)]
struct Setting {
    model: Option<String>,
}

fn setting_path(dir: &Path) -> PathBuf {
    dir.join("rag-entities.json")
}

/// The model the person chose for extraction; `None`: off (the default).
pub fn model(dir: &Path) -> Option<String> {
    let s: Setting = serde_json::from_slice(&std::fs::read(setting_path(dir)).ok()?).ok()?;
    s.model.filter(|m| crate::ragrerank::model_ok(m))
}

pub fn set_model(dir: &Path, m: Option<&str>) -> Result<(), String> {
    if let Some(m) = m {
        if !crate::ragrerank::model_ok(m) {
            return Err("that is not a model name".into());
        }
    }
    let bytes = serde_json::to_vec(&Setting { model: m.map(str::to_string) }).map_err(|e| e.to_string())?;
    iyakki_schema::write_atomic(&setting_path(dir), &bytes, 0o600)
}

/// Reads names out of one passage.
pub trait EntityModel: Send + Sync {
    fn name(&self) -> String;
    fn entities(&self, passage: &str) -> Result<Vec<String>, String>;
}

/// A chat model on the person's own Lemonade server (`POST <base>/chat/completions`, the OpenAI shape it serves).
pub struct HttpEntityModel {
    pub base: String,
    pub model: String,
    pub up: Arc<dyn Upstream>,
}

const INSTRUCTION: &str = "List the names of people, places, organisations, products and technical terms in the text the user sends. Answer with a JSON array of strings and nothing else. At most 24 names. If there are none, answer [].";

impl EntityModel for HttpEntityModel {
    fn name(&self) -> String {
        self.model.clone()
    }

    fn entities(&self, passage: &str) -> Result<Vec<String>, String> {
        let text: String = passage.chars().take(MAX_PASSAGE_CHARS).collect();
        let body = json!({"model": self.model, "temperature": 0, "max_tokens": 400, "stream": false, "messages": [{"role": "system", "content": INSTRUCTION}, {"role": "user", "content": text}]}).to_string();
        let url = format!("{}/chat/completions", self.base.trim_end_matches('/'));
        let r = self.up.request("POST", &url, &[("Content-Type".into(), "application/json".into())], body.as_bytes()).map_err(|e| format!("the model server did not answer: {e}"))?;
        if r.status != 200 {
            return Err(format!("the model server answered {}", r.status));
        }
        let v: Value = serde_json::from_slice(&r.body).map_err(|_| "the model's answer is not JSON".to_string())?;
        let content = v.pointer("/choices/0/message/content").and_then(Value::as_str).ok_or("the model's answer has no text")?;
        parse_entities(content)
    }
}

/// A model's answer as a clean list of names. The list may sit inside a code fence or after a sentence; anything that is not a JSON
/// array of strings is an error, and no name is kept that is empty, very long, or holds a control character.
pub fn parse_entities(content: &str) -> Result<Vec<String>, String> {
    let start = content.find('[').ok_or("the model did not answer with a list")?;
    let end = content.rfind(']').ok_or("the model did not answer with a list")?;
    if end < start {
        return Err("the model did not answer with a list".into());
    }
    let v: Value = serde_json::from_str(&content[start..=end]).map_err(|_| "the model's list is not valid".to_string())?;
    let arr = v.as_array().ok_or("the model's answer is not a list")?;
    let mut out: Vec<String> = Vec::new();
    for item in arr {
        let Some(s) = item.as_str() else { continue };
        let s = s.trim();
        if s.chars().count() < 2 || s.chars().count() > 60 || s.chars().any(char::is_control) {
            continue;
        }
        if !out.iter().any(|o| o.eq_ignore_ascii_case(s)) {
            out.push(s.to_string());
        }
        if out.len() >= MAX_PER_PASSAGE {
            break;
        }
    }
    Ok(out)
}

/// What a run would send: counted before anything is sent, and said on the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub passages: usize,
    pub chars: usize,
    /// The collection has more passages than one run sends.
    pub cut: bool,
}

pub fn plan(kb: &KnowledgeBase, ns: &str) -> Plan {
    let (mut passages, mut chars) = (0, 0);
    'all: for d in kb.docs(ns) {
        for (_, text) in kb.doc_chunks(ns, &d.id) {
            if passages >= MAX_PASSAGES {
                break 'all;
            }
            passages += 1;
            chars += text.chars().count().min(MAX_PASSAGE_CHARS);
        }
    }
    let total: usize = kb.docs(ns).iter().map(|d| kb.doc_chunks(ns, &d.id).len()).sum();
    Plan { passages, chars, cut: total > passages }
}

/// The names the model found, passage by passage.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Extracted {
    pub ns: String,
    pub docs: usize,
    pub chunks: usize,
    pub model: String,
    pub per: Vec<(String, u32, Vec<String>)>,
    pub failed: usize,
}

/// Run the model over the passages of `ns` (the caller already has the person's approval for exactly `plan`). A passage whose answer
/// fails is counted and skipped; when more than half fail the run stops with the reason, so a model that is not running costs one
/// error and not four hundred.
pub fn extract_all(kb: &KnowledgeBase, ns: &str, m: &dyn EntityModel) -> Result<Extracted, String> {
    let sig = kb.stats(ns).map(|s| (s.docs, s.chunks)).unwrap_or((0, 0));
    let mut out = Extracted { ns: ns.to_string(), docs: sig.0, chunks: sig.1, model: m.name(), ..Extracted::default() };
    let mut sent = 0usize;
    'all: for d in kb.docs(ns) {
        for (idx, text) in kb.doc_chunks(ns, &d.id) {
            if sent >= MAX_PASSAGES {
                break 'all;
            }
            sent += 1;
            match m.entities(&text) {
                Ok(e) if !e.is_empty() => out.per.push((d.id.clone(), idx, e)),
                Ok(_) => {}
                Err(e) => {
                    out.failed += 1;
                    if out.failed >= 5 && out.failed * 2 > sent {
                        return Err(format!("stopped: {e}"));
                    }
                }
            }
        }
    }
    Ok(out)
}

fn saved_path(dir: &Path, ns: &str) -> PathBuf {
    let safe: String = ns.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dir.join(format!("rag-entities-{safe}.cache.json"))
}

impl Extracted {
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        iyakki_schema::write_atomic(&saved_path(dir, &self.ns), &serde_json::to_vec(self).map_err(|e| e.to_string())?, 0o600)
    }

    /// The saved result of `ns`, only while the collection has the size it was made from.
    pub fn load(dir: &Path, ns: &str, sig: (usize, usize)) -> Option<Extracted> {
        let e: Extracted = serde_json::from_slice(&std::fs::read(saved_path(dir, ns)).ok()?).ok()?;
        (e.ns == ns && e.docs == sig.0 && e.chunks == sig.1).then_some(e)
    }

    pub fn forget(dir: &Path, ns: &str) {
        let _ = std::fs::remove_file(saved_path(dir, ns));
    }

    pub fn names(&self) -> usize {
        self.per.iter().map(|p| p.2.len()).sum()
    }
}
