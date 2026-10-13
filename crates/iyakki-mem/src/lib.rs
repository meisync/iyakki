//! Tiered agent memory, modeled on a CPU cache hierarchy.
//!
//! * **L1** — registers: a few named strings (system prompt pieces) under a hard byte budget.
//! * **L2** — scratchpad: the most recent notes, stored in a [`MemFs`]; oldest are paged out when full.
//! * **L3** — embedded vector store with cosine search for semantic recall of paged-out notes.
//!
//! Embeddings are pluggable via [`Embedder`]; the built-in [`HashEmbedder`] is a dependency-free
//! feature-hashing bag-of-words, good enough for lexical-semantic recall and for tests.

use std::collections::BTreeMap;

use iyakki_vfs::MemFs;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MemError {
    #[error("L1 budget exceeded ({limit} bytes)")]
    L1Full { limit: usize },
}

pub trait Embedder {
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// Hashes lowercase alphanumeric tokens into a fixed-size, L2-normalized vector.
pub struct HashEmbedder {
    pub dim: usize,
}

impl Default for HashEmbedder {
    fn default() -> Self {
        Self { dim: 256 }
    }
}

fn fnv1a(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Embedder for HashEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        for tok in text.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()) {
            let h = fnv1a(&tok.to_lowercase());
            let sign = if (h >> 63) & 1 == 0 { 1.0 } else { -1.0 };
            v[(h % self.dim as u64) as usize] += sign;
        }
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0.0 {
            v.iter_mut().for_each(|x| *x /= n);
        }
        v
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let (na, nb) = (a.iter().map(|x| x * x).sum::<f32>().sqrt(), b.iter().map(|x| x * x).sum::<f32>().sqrt());
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na * nb) }
}

/// L3: brute-force cosine vector store.
#[derive(Default)]
pub struct VectorStore {
    items: Vec<(String, Vec<f32>)>,
}

impl VectorStore {
    pub fn insert(&mut self, text: String, vec: Vec<f32>) {
        self.items.push((text, vec));
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn search(&self, q: &[f32], k: usize) -> Vec<(&str, f32)> {
        let mut s: Vec<(&str, f32)> = self.items.iter().map(|(t, v)| (t.as_str(), cosine(q, v))).collect();
        s.sort_by(|a, b| b.1.total_cmp(&a.1));
        s.truncate(k);
        s
    }
    pub fn remove(&mut self, text: &str) {
        self.items.retain(|(t, _)| t != text);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    L2,
    L3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub text: String,
    pub tier: Tier,
    pub score: f32,
}

pub struct TieredMemory<E: Embedder = HashEmbedder> {
    l1: BTreeMap<String, String>,
    l1_budget: usize,
    l2: MemFs,
    l2_cap: usize,
    seq: u64,
    l3: VectorStore,
    embedder: E,
}

impl TieredMemory<HashEmbedder> {
    pub fn new(l1_budget: usize, l2_cap: usize) -> Self {
        Self::with_embedder(l1_budget, l2_cap, HashEmbedder::default())
    }
}

impl<E: Embedder> TieredMemory<E> {
    pub fn with_embedder(l1_budget: usize, l2_cap: usize, embedder: E) -> Self {
        Self { l1: BTreeMap::new(), l1_budget, l2: MemFs::new(), l2_cap: l2_cap.max(1), seq: 0, l3: VectorStore::default(), embedder }
    }

    // ---- L1 ----
    pub fn l1_set(&mut self, key: &str, value: &str) -> Result<(), MemError> {
        let used: usize = self.l1.iter().filter(|(k, _)| k.as_str() != key).map(|(k, v)| k.len() + v.len()).sum();
        if used + key.len() + value.len() > self.l1_budget {
            return Err(MemError::L1Full { limit: self.l1_budget });
        }
        self.l1.insert(key.into(), value.into());
        Ok(())
    }
    pub fn l1_get(&self, key: &str) -> Option<&str> {
        self.l1.get(key).map(String::as_str)
    }
    /// Concatenated registers, in key order, ready to prepend to a prompt.
    pub fn l1_render(&self) -> String {
        self.l1.iter().map(|(k, v)| format!("[{k}]\n{v}\n")).collect()
    }

    // ---- L2 / L3 ----
    fn l2_entries(&self) -> Vec<(String, String)> {
        self.l2
            .list("/scratch")
            .unwrap_or_default()
            .into_iter()
            .map(|n| {
                let t = String::from_utf8_lossy(&self.l2.read(&format!("/scratch/{n}")).unwrap_or_default()).into_owned();
                (n, t)
            })
            .collect()
    }

    pub fn l2_len(&self) -> usize {
        self.l2.list("/scratch").map_or(0, |v| v.len())
    }
    pub fn l3_len(&self) -> usize {
        self.l3.len()
    }

    /// Store a note in L2; if L2 is over capacity the oldest notes are paged out to L3.
    pub fn remember(&mut self, text: &str) {
        self.put_l2(text);
        while self.l2_len() > self.l2_cap {
            let oldest = self.l2_entries().into_iter().next().expect("non-empty");
            let _ = self.l2.remove(&format!("/scratch/{}", oldest.0));
            let v = self.embedder.embed(&oldest.1);
            self.l3.insert(oldest.1, v);
        }
    }

    fn put_l2(&mut self, text: &str) {
        self.seq += 1;
        let _ = self.l2.write(&format!("/scratch/{:012}", self.seq), text.as_bytes());
    }

    /// Semantic recall across L2 and L3, best first. The top L3 hit is promoted back into L2.
    pub fn recall(&mut self, query: &str, k: usize) -> Vec<Hit> {
        let q = self.embedder.embed(query);
        let mut hits: Vec<Hit> = self
            .l2_entries()
            .into_iter()
            .map(|(_, t)| Hit { score: cosine(&q, &self.embedder.embed(&t)), text: t, tier: Tier::L2 })
            .collect();
        hits.extend(self.l3.search(&q, k).into_iter().map(|(t, s)| Hit { text: t.into(), tier: Tier::L3, score: s }));
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(k);
        if let Some(top) = hits.first().filter(|h| h.tier == Tier::L3 && h.score > 0.0).cloned() {
            self.l3.remove(&top.text);
            self.remember(&top.text);
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l1_budget() {
        let mut m = TieredMemory::new(20, 4);
        m.l1_set("sys", "be brief").unwrap();
        assert_eq!(m.l1_get("sys"), Some("be brief"));
        assert_eq!(m.l1_set("big", "this is far too large for it"), Err(MemError::L1Full { limit: 20 }));
        m.l1_set("sys", "be terse").unwrap(); // overwrite doesn't double count
        assert!(m.l1_render().contains("[sys]\nbe terse"));
    }

    #[test]
    fn pages_out_and_recalls_semantically() {
        let mut m = TieredMemory::new(100, 2);
        m.remember("the deploy key lives in the vault under prod");
        m.remember("user prefers dark mode and tabs");
        m.remember("meeting notes about quarterly budget");
        m.remember("lunch order was a sandwich");
        assert_eq!(m.l2_len(), 2);
        assert_eq!(m.l3_len(), 2);
        let hits = m.recall("where is the deploy key vault", 2);
        assert_eq!(hits[0].text, "the deploy key lives in the vault under prod");
        assert_eq!(hits[0].tier, Tier::L3);
        // promoted back to L2, total conserved
        assert_eq!(m.l2_len() + m.l3_len(), 4);
        let again = m.recall("deploy key vault", 1);
        assert_eq!(again[0].tier, Tier::L2);
    }

    #[test]
    fn embedder_similarity_ordering() {
        let e = HashEmbedder::default();
        let a = e.embed("rust wasm sandbox");
        assert!(cosine(&a, &e.embed("wasm sandbox in rust")) > cosine(&a, &e.embed("banana bread recipe")));
        assert_eq!(e.embed("").iter().sum::<f32>(), 0.0);
    }
}
