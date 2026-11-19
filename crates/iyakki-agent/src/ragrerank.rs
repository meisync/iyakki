//! Re-ranking of retrieved passages (F-RAG-14): a second step that orders the passages the search found before they reach the answer.
//!
//! Three modes, kept per machine in `<data>/rag-rerank.json` (off by default, so nothing changes until the person chooses):
//! * `off`: the search order stands.
//! * `lexical`: an in-house scorer that looks at how many of the question's words a passage has, whether they stand together in the
//!   question's order, and how close they sit; no model, no network.
//! * `model`: a reranking model served by the person's Lemonade (`POST <base>/reranking` with the question and the passages, answered
//!   with a relevance score for each); when the server does not answer, the search order stands and the answer carries a warning.
//!
//! Only the order of passages the search already found changes; nothing new is fetched and nothing is dropped, so a mistake of the
//! second step can at worst put a good passage lower, never hide it from `rag_search` with a larger `k`.

pub use iyakki_shared::ragrerank::{Mode, model_ok};
use std::sync::Arc;

use iyakki_proxy::Upstream;
use serde_json::{json, Value};

use crate::kb::tokenize;

/// Passages the second step looks at (the best of the search), at most.
pub const MAX_CANDIDATES: usize = 40;




/// Scores passages for a question: higher is more relevant; one score per passage, in the order given.
pub trait Reranker: Send + Sync {
    fn name(&self) -> String;
    fn scores(&self, query: &str, passages: &[&str]) -> Result<Vec<f32>, String>;
}

/// The in-house scorer: 0.5 for the share of the question's different words the passage has, 0.3 for the longest run of them in the
/// question's own order, 0.2 for how tightly all the found words sit together.
pub struct LexicalReranker;

impl Reranker for LexicalReranker {
    fn name(&self) -> String {
        "lexical".into()
    }

    fn scores(&self, query: &str, passages: &[&str]) -> Result<Vec<f32>, String> {
        let q = tokenize(query);
        if q.is_empty() {
            return Err("the question has no words".into());
        }
        let mut uniq: Vec<&String> = Vec::new();
        for t in &q {
            if !uniq.contains(&t) {
                uniq.push(t);
            }
        }
        Ok(passages
            .iter()
            .map(|p| {
                let toks = tokenize(p);
                if toks.is_empty() {
                    return 0.0;
                }
                let have: Vec<&&String> = uniq.iter().filter(|u| toks.contains(**u)).collect();
                let coverage = have.len() as f32 / uniq.len() as f32;
                // the longest stretch of the question's words that also stands in the passage, in the same order
                let mut best = 0usize;
                for s in 0..q.len() {
                    let mut len = 0;
                    'grow: for l in 1..=(q.len() - s) {
                        let ph = &q[s..s + l];
                        if toks.windows(l).any(|w| w == ph) {
                            len = l;
                        } else {
                            break 'grow;
                        }
                    }
                    best = best.max(len);
                }
                let phrase = best as f32 / q.len() as f32;
                // the smallest stretch of the passage that holds every found word
                let proximity = if have.len() <= 1 {
                    if have.is_empty() { 0.0 } else { 1.0 / (1.0 + (toks.len() as f32).ln()) }
                } else {
                    let mut at: Vec<(usize, usize)> = Vec::new();
                    for (i, t) in toks.iter().enumerate() {
                        if let Some(k) = have.iter().position(|h| **h == t) {
                            at.push((i, k));
                        }
                    }
                    let need = have.len();
                    let mut counts = vec![0usize; need];
                    let (mut have_n, mut lo, mut min_w) = (0usize, 0usize, usize::MAX);
                    for hi in 0..at.len() {
                        if counts[at[hi].1] == 0 {
                            have_n += 1;
                        }
                        counts[at[hi].1] += 1;
                        while have_n == need {
                            min_w = min_w.min(at[hi].0 - at[lo].0 + 1);
                            counts[at[lo].1] -= 1;
                            if counts[at[lo].1] == 0 {
                                have_n -= 1;
                            }
                            lo += 1;
                        }
                    }
                    if min_w == usize::MAX { 0.0 } else { need as f32 / min_w as f32 }
                };
                0.5 * coverage + 0.3 * phrase + 0.2 * proximity.min(1.0)
            })
            .collect())
    }
}

/// A reranking model behind an HTTP service (Lemonade's `/reranking`): `{"model","query","documents"}` answered by
/// `{"results":[{"index","relevance_score"}]}`.
pub struct HttpReranker {
    pub base: String,
    pub model: String,
    pub up: Arc<dyn Upstream>,
}

impl Reranker for HttpReranker {
    fn name(&self) -> String {
        format!("model {}", self.model)
    }

    fn scores(&self, query: &str, passages: &[&str]) -> Result<Vec<f32>, String> {
        let body = json!({"model": self.model, "query": query, "documents": passages}).to_string();
        let url = format!("{}/reranking", self.base.trim_end_matches('/'));
        let r = self.up.request("POST", &url, &[("Content-Type".into(), "application/json".into())], body.as_bytes()).map_err(|e| format!("the reranking server did not answer: {e}"))?;
        if r.status != 200 {
            return Err(format!("the reranking server answered {}", r.status));
        }
        parse_scores(&String::from_utf8_lossy(&r.body), passages.len())
    }
}

/// Reads the answer of a reranking service: one score per document, by `index`.
pub fn parse_scores(body: &str, n: usize) -> Result<Vec<f32>, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "the reranking answer is not JSON".to_string())?;
    let items = v.get("results").or_else(|| v.get("data")).and_then(Value::as_array).ok_or("the reranking answer has no results")?;
    let mut out = vec![f32::NEG_INFINITY; n];
    for it in items {
        let i = it.get("index").and_then(Value::as_u64).ok_or("a result has no index")? as usize;
        let s = it.get("relevance_score").or_else(|| it.get("score")).and_then(Value::as_f64).ok_or("a result has no score")? as f32;
        if i >= n || !s.is_finite() {
            return Err("a result names a document that was not sent, or a score that is not a number".into());
        }
        out[i] = s;
    }
    if out.iter().any(|s| *s == f32::NEG_INFINITY) {
        return Err("the reranking answer does not score every document".into());
    }
    Ok(out)
}

/// The reranker the mode asks for. `base` is where the local reranking server is (looked up when the mode is `model`).
pub fn build(mode: &Mode, base: Option<String>, up: Arc<dyn Upstream>) -> Result<Option<Box<dyn Reranker>>, String> {
    match mode {
        Mode::Off => Ok(None),
        Mode::Lexical => Ok(Some(Box::new(LexicalReranker))),
        Mode::Model { model } => {
            let base = base.ok_or("no Lemonade server answers on this computer, so the passages stay in the search order")?;
            Ok(Some(Box::new(HttpReranker { base, model: model.clone(), up })))
        }
    }
}

/// Order `items` by the reranker. `text` gives the passage text of an item. Items after `MAX_CANDIDATES` keep their place behind the
/// reordered ones. Ties keep the search order. An error leaves the order alone and is returned for a warning.
pub fn rerank<T>(rr: &dyn Reranker, query: &str, items: Vec<T>, text: impl Fn(&T) -> String) -> (Vec<T>, Option<String>) {
    let n = items.len().min(MAX_CANDIDATES);
    if n < 2 {
        return (items, None);
    }
    let texts: Vec<String> = items[..n].iter().map(&text).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    match rr.scores(query, &refs) {
        Ok(s) => {
            let mut idx: Vec<usize> = (0..n).collect();
            idx.sort_by(|a, b| s[*b].partial_cmp(&s[*a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b)));
            let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
            let mut out: Vec<T> = idx.into_iter().filter_map(|i| slots[i].take()).collect();
            out.extend(slots.into_iter().flatten());
            (out, None)
        }
        Err(e) => (items, Some(format!("re-ranking ({}) was skipped: {e}", rr.name()))),
    }
}

#[cfg(test)]
#[path = "ragrerank_tests.rs"]
mod tests;
