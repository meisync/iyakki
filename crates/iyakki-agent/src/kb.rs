//! Embedded knowledge base: namespaces of documents split into chunks, searchable by a hybrid of
//! BM25 keyword ranking and vector similarity (fused with Reciprocal Rank Fusion, then
//! de-duplicated with MMR). Pure Rust, no server process, persisted to disk.
//!
//! Namespaces partition data and access: `user/<name>`, `shared`, `session/<id>`,
//! `skills/shared`, `skills/user/<name>`. See [`crate::access`] for who may use which.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_ledger::hash_bytes;
use iyakki_mem::cosine;
use serde::{Deserialize, Serialize};

use crate::embed::{HashEmbed, TextEmbedder};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KbError {
    #[error("invalid namespace name")]
    BadNamespace,
    #[error("no such namespace")]
    NoSuchNamespace,
    #[error("no such document")]
    NoSuchDoc,
    #[error("document is empty")]
    Empty,
    #[error("document too large (limit {0} bytes)")]
    TooLarge(usize),
    #[error("namespace is full (limit {0} chunks)")]
    NamespaceFull(usize),
    #[error("storage error: {0}")]
    Storage(String),
}

pub const MAX_DOC_BYTES: usize = 2 << 20;
pub const MAX_CHUNKS_PER_NS: usize = 100_000;

/// `a/b/c`: 1–4 segments of `[a-z0-9][a-z0-9._-]{0,63}`.
pub fn valid_namespace(ns: &str) -> bool {
    let segs: Vec<&str> = ns.split('/').collect();
    (1..=4).contains(&segs.len())
        && segs.iter().all(|s| {
            let mut c = s.chars();
            c.next().is_some_and(|f| f.is_ascii_lowercase() || f.is_ascii_digit()) && s.len() <= 64 && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || matches!(x, '.' | '_' | '-'))
        })
        && !ns.contains("..")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocInfo {
    pub id: String,
    pub title: String,
    pub source: String,
    pub added: u64,
    pub chunks: usize,
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
}

struct Chunk {
    doc: String,
    idx: u32,
    text: String,
    vec: Vec<f32>, // empty if embedding was unavailable at ingest time
    tf: HashMap<String, u32>,
    len: u32,
}

struct Namespace {
    embedder_id: String,
    docs: BTreeMap<String, DocInfo>,
    chunks: Vec<Chunk>,
    df: HashMap<String, u32>,
    total_len: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub ns: String,
    pub doc_id: String,
    pub title: String,
    pub source: String,
    pub chunk_idx: u32,
    pub text: String,
    /// Final fused score (higher is better; only meaningful relative to other hits of one query).
    pub score: f32,
    pub bm25: f32,
    pub vector: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub docs: usize,
    pub chunks: usize,
    pub unembedded: usize,
}

// ------------------------------------------------------------------ text processing

const STOP: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "had", "her", "was", "one", "our", "out", "has", "have", "from", "that", "this", "with", "what", "when", "where", "which", "who", "how", "why", "does", "did", "its", "into", "than", "then", "them", "they", "will", "would", "there", "their", "about",
];

fn stem(w: &str) -> String {
    // plural "s": keys -> key, but not boss/virus/analysis
    if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") && !w.ends_with("es") {
        return w[..w.len() - 1].to_string();
    }
    for suf in ["ing", "edly", "ed", "es"] {
        if w.len() > suf.len() + 3 && w.ends_with(suf) {
            return w[..w.len() - suf.len()].to_string();
        }
    }
    w.to_string()
}

/// Lowercase alphanumeric tokens, stop words dropped, lightly stemmed.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .map(str::to_lowercase)
        .filter(|t| !STOP.contains(&t.as_str()))
        .map(|t| stem(&t))
        .collect()
}

/// Split into overlapping chunks of at most `max` bytes, preferring paragraph then sentence
/// boundaries. Each chunk after the first begins with up to `overlap` bytes of the previous one
/// (cut at a word boundary) so facts spanning a boundary stay retrievable.
pub fn chunk_text(text: &str, max: usize, overlap: usize) -> Vec<String> {
    let max = max.max(50);
    // 1. break into atoms no longer than `max`: paragraphs, else sentences, else hard splits
    let mut atoms: Vec<String> = Vec::new();
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if para.len() <= max {
            atoms.push(para.to_string());
            continue;
        }
        let mut cur = String::new();
        let mut pieces: Vec<String> = Vec::new();
        for sent in para.split_inclusive(['.', '!', '?', '\n']) {
            if cur.len() + sent.len() > max && !cur.is_empty() {
                pieces.push(std::mem::take(&mut cur));
            }
            if sent.len() > max {
                let mut piece = String::new();
                for ch in sent.chars() {
                    if piece.len() + ch.len_utf8() > max {
                        pieces.push(std::mem::take(&mut piece));
                    }
                    piece.push(ch);
                }
                cur = piece;
            } else {
                cur.push_str(sent);
            }
        }
        if !cur.is_empty() {
            pieces.push(cur);
        }
        atoms.extend(pieces.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()));
    }
    // 2. pack atoms into chunks
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for a in atoms {
        if !cur.is_empty() && cur.len() + a.len() + 2 > max {
            chunks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(&a);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    // 3. overlap (kept within `max`)
    if overlap == 0 || chunks.len() < 2 {
        return chunks;
    }
    let mut out = vec![chunks[0].clone()];
    for i in 1..chunks.len() {
        let prev = &chunks[i - 1];
        let mut start = prev.len().saturating_sub(overlap);
        while start < prev.len() && !prev.is_char_boundary(start) {
            start += 1;
        }
        let tail = &prev[start..];
        let tail = tail.find(char::is_whitespace).map_or(tail, |p| tail[p..].trim_start());
        if tail.is_empty() || tail.len() + chunks[i].len() + 1 > max {
            out.push(chunks[i].clone());
        } else {
            out.push(format!("{tail} {}", chunks[i]));
        }
    }
    out
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ------------------------------------------------------------------ the knowledge base

pub struct KnowledgeBase {
    dir: Option<PathBuf>,
    embedder: Arc<dyn TextEmbedder>,
    ns: BTreeMap<String, Namespace>,
    /// Files that failed to load (corrupt/unsupported) — surfaced instead of silently dropped.
    pub load_errors: Vec<String>,
}

impl KnowledgeBase {
    /// In-memory only (tests, ephemeral sessions).
    pub fn in_memory(embedder: Arc<dyn TextEmbedder>) -> KnowledgeBase {
        KnowledgeBase { dir: None, embedder, ns: BTreeMap::new(), load_errors: vec![] }
    }

    /// Persistent: loads every `*.afkb` under `dir`, saves after each change.
    pub fn open(dir: &Path, embedder: Arc<dyn TextEmbedder>) -> KnowledgeBase {
        let mut kb = KnowledgeBase { dir: Some(dir.into()), embedder, ns: BTreeMap::new(), load_errors: vec![] };
        let _ = std::fs::create_dir_all(dir);
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "afkb")).collect();
            files.sort();
            for f in files {
                match std::fs::read(&f).map_err(|e| e.to_string()).and_then(|b| decode_namespace(&b)) {
                    Ok((name, ns)) => {
                        kb.ns.insert(name, ns);
                    }
                    Err(e) => kb.load_errors.push(format!("{}: {e}", f.display())),
                }
            }
        }
        kb
    }

    pub fn embedder_id(&self) -> String {
        self.embedder.id()
    }

    pub fn namespaces(&self) -> Vec<String> {
        self.ns.keys().cloned().collect()
    }

    pub fn stats(&self, ns: &str) -> Option<Stats> {
        self.ns.get(ns).map(|n| Stats { docs: n.docs.len(), chunks: n.chunks.len(), unembedded: n.chunks.iter().filter(|c| c.vec.is_empty()).count() })
    }

    pub fn docs(&self, ns: &str) -> Vec<DocInfo> {
        self.ns.get(ns).map(|n| n.docs.values().cloned().collect()).unwrap_or_default()
    }

    fn file_for(&self, ns: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{}.afkb", ns.replace('/', "__"))))
    }

    fn save(&self, ns: &str) -> Result<(), KbError> {
        let (Some(path), Some(n)) = (self.file_for(ns), self.ns.get(ns)) else { return Ok(()) };
        let bytes = encode_namespace(ns, n);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| KbError::Storage(e.to_string()))
    }

    fn ensure_ns(&mut self, ns: &str) -> Result<&mut Namespace, KbError> {
        if !valid_namespace(ns) {
            return Err(KbError::BadNamespace);
        }
        let id = self.embedder.id();
        Ok(self.ns.entry(ns.to_string()).or_insert_with(|| Namespace { embedder_id: id, docs: BTreeMap::new(), chunks: vec![], df: HashMap::new(), total_len: 0 }))
    }

    /// Add a document. Identical content already in the namespace is not added twice (its existing
    /// id is returned). Returns `(doc_id, newly_added)`.
    pub fn add_document(&mut self, ns: &str, title: &str, source: &str, text: &str, meta: BTreeMap<String, String>) -> Result<(String, bool), KbError> {
        if text.trim().is_empty() {
            return Err(KbError::Empty);
        }
        if text.len() > MAX_DOC_BYTES {
            return Err(KbError::TooLarge(MAX_DOC_BYTES));
        }
        if !valid_namespace(ns) {
            return Err(KbError::BadNamespace);
        }
        let id = hex(&hash_bytes(text.as_bytes())[..8]);
        if self.ns.get(ns).is_some_and(|n| n.docs.contains_key(&id)) {
            return Ok((id, false));
        }
        let pieces = chunk_text(text, 700, 100);
        if self.ns.get(ns).map_or(0, |n| n.chunks.len()) + pieces.len() > MAX_CHUNKS_PER_NS {
            return Err(KbError::NamespaceFull(MAX_CHUNKS_PER_NS));
        }
        // Embed outside the namespace borrow; if the embedder is down we still index for keywords.
        let embedder_id = self.embedder.id();
        let vecs = self.embedder.embed(&pieces).ok().filter(|v| v.len() == pieces.len());
        let n = self.ensure_ns(ns)?;
        // A namespace built with a different embedder can't take these vectors: keep them empty.
        let usable = vecs.filter(|_| n.embedder_id == embedder_id);
        for (i, text) in pieces.iter().enumerate() {
            let toks = tokenize(text);
            let mut tf: HashMap<String, u32> = HashMap::new();
            for t in &toks {
                *tf.entry(t.clone()).or_default() += 1;
            }
            for t in tf.keys() {
                *n.df.entry(t.clone()).or_default() += 1;
            }
            n.total_len += toks.len() as u64;
            n.chunks.push(Chunk { doc: id.clone(), idx: i as u32, text: text.clone(), vec: usable.as_ref().map(|v| v[i].clone()).unwrap_or_default(), tf, len: toks.len() as u32 });
        }
        n.docs.insert(id.clone(), DocInfo { id: id.clone(), title: title.chars().take(120).collect(), source: source.chars().take(240).collect(), added: now(), chunks: pieces.len(), meta });
        self.save(ns)?;
        Ok((id, true))
    }

    /// Add several documents; returns per-item results so one bad file doesn't abort the batch.
    pub fn add_documents(&mut self, ns: &str, items: &[(String, String, String)]) -> Vec<Result<(String, bool), KbError>> {
        items.iter().map(|(title, source, text)| self.add_document(ns, title, source, text, BTreeMap::new())).collect()
    }

    /// Ingest text files under `dir` (recursive, symlinks not followed, bounded count and size).
    pub fn ingest_dir(&mut self, ns: &str, dir: &Path) -> (usize, usize) {
        fn walk(d: &Path, out: &mut Vec<PathBuf>, depth: u32) {
            if depth > 6 || out.len() >= 2000 {
                return;
            }
            let Ok(rd) = std::fs::read_dir(d) else { return };
            for e in rd.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_symlink() {
                    continue;
                }
                if ft.is_dir() {
                    walk(&e.path(), out, depth + 1);
                } else if ft.is_file() {
                    out.push(e.path());
                }
            }
        }
        const EXT: &[&str] = &["txt", "md", "rst", "log", "csv", "json", "toml", "yaml", "yml", "rs", "py", "c", "h", "sh"];
        let mut files = Vec::new();
        walk(dir, &mut files, 0);
        files.sort();
        let (mut added, mut seen) = (0, 0);
        for f in files {
            let ok = f.extension().and_then(|e| e.to_str()).is_some_and(|e| EXT.contains(&e.to_ascii_lowercase().as_str()));
            if !ok || std::fs::metadata(&f).map_or(true, |m| m.len() as usize > MAX_DOC_BYTES) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            let rel = f.strip_prefix(dir).unwrap_or(&f).display().to_string();
            seen += 1;
            if let Ok((_, true)) = self.add_document(ns, &rel, &rel, &text, BTreeMap::new()) {
                added += 1;
            }
        }
        (added, seen)
    }

    pub fn remove_document(&mut self, ns: &str, doc_id: &str) -> Result<(), KbError> {
        let n = self.ns.get_mut(ns).ok_or(KbError::NoSuchNamespace)?;
        n.docs.remove(doc_id).ok_or(KbError::NoSuchDoc)?;
        n.chunks.retain(|c| c.doc != doc_id);
        rebuild_stats(n);
        self.save(ns)
    }

    pub fn drop_namespace(&mut self, ns: &str) -> Result<(), KbError> {
        self.ns.remove(ns).ok_or(KbError::NoSuchNamespace)?;
        if let Some(f) = self.file_for(ns) {
            let _ = std::fs::remove_file(f);
        }
        Ok(())
    }

    /// Re-embed a namespace with the current embedder (after switching models, or to fill chunks
    /// that were indexed while the embedding service was unreachable).
    pub fn reindex(&mut self, ns: &str) -> Result<(), KbError> {
        let texts: Vec<String> = self.ns.get(ns).ok_or(KbError::NoSuchNamespace)?.chunks.iter().map(|c| c.text.clone()).collect();
        let vecs = self.embedder.embed(&texts).map_err(KbError::Storage)?;
        if vecs.len() != texts.len() {
            return Err(KbError::Storage("embedder returned the wrong number of vectors".into()));
        }
        let id = self.embedder.id();
        let n = self.ns.get_mut(ns).ok_or(KbError::NoSuchNamespace)?;
        for (c, v) in n.chunks.iter_mut().zip(vecs) {
            c.vec = v;
        }
        n.embedder_id = id;
        self.save(ns)
    }

    /// Namespaces whose chunks were embedded by something other than the embedder in use now (the person switched
    /// embedding models), or that have chunks with no vector at all. Search still works on them (by keywords) but not as well.
    pub fn stale_namespaces(&self) -> Vec<String> {
        let now = self.embedder.id();
        self.ns.iter().filter(|(_, n)| !n.chunks.is_empty() && (n.embedder_id != now || n.chunks.iter().any(|c| c.vec.is_empty()))).map(|(k, _)| k.clone()).collect()
    }

    /// Re-embed every stale namespace; returns how many were done and the first problem met (the others are still tried).
    pub fn reindex_stale(&mut self) -> (usize, Option<String>) {
        let (mut done, mut first_err) = (0, None);
        for ns in self.stale_namespaces() {
            match self.reindex(&ns) {
                Ok(()) => done += 1,
                Err(e) => {
                    first_err.get_or_insert(format!("{ns}: {e:?}"));
                }
            }
        }
        (done, first_err)
    }

    // ------------------------------------------------------------ search

    /// Hybrid search over `namespaces` (unknown ones are skipped). `weights` optionally boosts a
    /// namespace's fused scores (e.g. recent conversation memory).
    pub fn search(&self, namespaces: &[&str], query: &str, k: usize) -> Vec<Hit> {
        self.search_weighted(&namespaces.iter().map(|n| (*n, 1.0f32)).collect::<Vec<_>>(), query, k)
    }

    pub fn search_weighted(&self, namespaces: &[(&str, f32)], query: &str, k: usize) -> Vec<Hit> {
        let qtoks = tokenize(query);
        if k == 0 || (qtoks.is_empty() && query.trim().is_empty()) {
            return vec![];
        }
        let qvec = self.embedder.embed(&[query.to_string()]).ok().and_then(|mut v| v.pop());
        struct Cand<'a> {
            ns: &'a str,
            chunk: &'a Chunk,
            weight: f32,
            bm25: f32,
            vector: f32,
        }
        let mut cands: Vec<Cand> = Vec::new();
        for (name, weight) in namespaces {
            let Some((nm, n)) = self.ns.get_key_value(*name) else { continue };
            let nchunks = n.chunks.len() as f32;
            if nchunks == 0.0 {
                continue;
            }
            let avg_len = (n.total_len as f32 / nchunks).max(1.0);
            let vec_ok = n.embedder_id == self.embedder.id();
            for c in &n.chunks {
                let mut bm = 0.0;
                for t in &qtoks {
                    if let Some(&f) = c.tf.get(t) {
                        let df = *n.df.get(t).unwrap_or(&1) as f32;
                        let idf = (1.0 + (nchunks - df + 0.5) / (df + 0.5)).ln();
                        let f = f as f32;
                        bm += idf * f * 2.2 / (f + 1.2 * (1.0 - 0.75 + 0.75 * c.len as f32 / avg_len));
                    }
                }
                let vs = match (&qvec, vec_ok && !c.vec.is_empty() && qvec.as_ref().is_some_and(|q| q.len() == c.vec.len())) {
                    (Some(q), true) => cosine(q, &c.vec),
                    _ => 0.0,
                };
                if bm > 0.0 || vs > 0.0 {
                    cands.push(Cand { ns: nm, chunk: c, weight: *weight, bm25: bm, vector: vs });
                }
            }
        }
        if cands.is_empty() {
            return vec![];
        }
        // Reciprocal Rank Fusion over the keyword ranking and the vector ranking.
        const RRF_K: f32 = 60.0;
        let mut fused = vec![0.0f32; cands.len()];
        let mut by_bm: Vec<usize> = (0..cands.len()).filter(|&i| cands[i].bm25 > 0.0).collect();
        by_bm.sort_by(|&a, &b| cands[b].bm25.total_cmp(&cands[a].bm25));
        for (rank, &i) in by_bm.iter().take(100).enumerate() {
            fused[i] += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
        let mut by_vec: Vec<usize> = (0..cands.len()).filter(|&i| cands[i].vector > 0.0).collect();
        by_vec.sort_by(|&a, &b| cands[b].vector.total_cmp(&cands[a].vector));
        for (rank, &i) in by_vec.iter().take(100).enumerate() {
            fused[i] += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
        for (i, c) in cands.iter().enumerate() {
            fused[i] *= c.weight;
        }
        let mut order: Vec<usize> = (0..cands.len()).filter(|&i| fused[i] > 0.0).collect();
        order.sort_by(|&a, &b| fused[b].total_cmp(&fused[a]).then(cands[a].chunk.idx.cmp(&cands[b].chunk.idx)));
        // MMR: prefer relevant chunks that are not near-duplicates of ones already picked.
        let pool: Vec<usize> = order.into_iter().take(k * 4 + 10).collect();
        let maxf = pool.first().map_or(1.0, |&i| fused[i]).max(1e-9);
        let mut picked: Vec<usize> = Vec::new();
        let mut rest = pool;
        while picked.len() < k && !rest.is_empty() {
            let mut best = (0usize, f32::MIN);
            for (pos, &i) in rest.iter().enumerate() {
                let rel = fused[i] / maxf;
                let redundancy = picked.iter().map(|&p| if cands[i].chunk.vec.is_empty() || cands[p].chunk.vec.is_empty() { 0.0 } else { cosine(&cands[i].chunk.vec, &cands[p].chunk.vec) }).fold(0.0f32, f32::max);
                let mmr = 0.75 * rel - 0.25 * redundancy;
                if mmr > best.1 {
                    best = (pos, mmr);
                }
            }
            picked.push(rest.remove(best.0));
        }
        picked
            .into_iter()
            .map(|i| {
                let c = &cands[i];
                let d = self.ns[c.ns].docs.get(&c.chunk.doc);
                Hit {
                    ns: c.ns.to_string(),
                    doc_id: c.chunk.doc.clone(),
                    title: d.map(|d| d.title.clone()).unwrap_or_default(),
                    source: d.map(|d| d.source.clone()).unwrap_or_default(),
                    chunk_idx: c.chunk.idx,
                    text: c.chunk.text.clone(),
                    score: fused[i],
                    bm25: c.bm25,
                    vector: c.vector,
                }
            })
            .collect()
    }
}

fn rebuild_stats(n: &mut Namespace) {
    n.df.clear();
    n.total_len = 0;
    for c in &n.chunks {
        for t in c.tf.keys() {
            *n.df.entry(t.clone()).or_default() += 1;
        }
        n.total_len += c.len as u64;
    }
}

// ------------------------------------------------------------------ persistence

const MAGIC: &[u8; 5] = b"AFKB1";

#[derive(Serialize, Deserialize)]
struct Manifest {
    name: String,
    embedder_id: String,
    docs: Vec<DocInfo>,
}

fn encode_namespace(name: &str, n: &Namespace) -> Vec<u8> {
    let m = serde_json::to_vec(&Manifest { name: name.into(), embedder_id: n.embedder_id.clone(), docs: n.docs.values().cloned().collect() }).unwrap_or_default();
    let mut o = MAGIC.to_vec();
    o.extend((m.len() as u32).to_le_bytes());
    o.extend(&m);
    o.extend((n.chunks.len() as u32).to_le_bytes());
    for c in &n.chunks {
        o.extend((c.doc.len() as u16).to_le_bytes());
        o.extend(c.doc.as_bytes());
        o.extend(c.idx.to_le_bytes());
        o.extend((c.text.len() as u32).to_le_bytes());
        o.extend(c.text.as_bytes());
        o.extend((c.vec.len() as u32).to_le_bytes());
        for f in &c.vec {
            o.extend(f.to_le_bytes());
        }
    }
    o
}

fn decode_namespace(b: &[u8]) -> Result<(String, Namespace), String> {
    struct R<'a>(&'a [u8]);
    impl<'a> R<'a> {
        fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
            if self.0.len() < n {
                return Err("truncated file".into());
            }
            let (a, b) = self.0.split_at(n);
            self.0 = b;
            Ok(a)
        }
        fn u32(&mut self) -> Result<u32, String> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
        }
    }
    let mut r = R(b);
    if r.take(5)? != MAGIC {
        return Err("not an IYAKKI knowledge-base file".into());
    }
    let ml = r.u32()? as usize;
    let m: Manifest = serde_json::from_slice(r.take(ml)?).map_err(|e| format!("bad manifest: {e}"))?;
    if !valid_namespace(&m.name) {
        return Err("bad namespace name in file".into());
    }
    let nchunks = r.u32()? as usize;
    if nchunks > MAX_CHUNKS_PER_NS {
        return Err("too many chunks".into());
    }
    let mut chunks = Vec::with_capacity(nchunks.min(4096));
    for _ in 0..nchunks {
        let dl = u16::from_le_bytes(r.take(2)?.try_into().unwrap()) as usize;
        let doc = String::from_utf8(r.take(dl)?.to_vec()).map_err(|_| "bad utf8")?;
        let idx = r.u32()?;
        let tl = r.u32()? as usize;
        let text = String::from_utf8(r.take(tl)?.to_vec()).map_err(|_| "bad utf8")?;
        let vl = r.u32()? as usize;
        if vl > 8192 {
            return Err("implausible vector size".into());
        }
        let vec: Vec<f32> = r.take(vl * 4)?.chunks_exact(4).map(|x| f32::from_le_bytes(x.try_into().unwrap())).collect();
        let toks = tokenize(&text);
        let mut tf: HashMap<String, u32> = HashMap::new();
        for t in &toks {
            *tf.entry(t.clone()).or_default() += 1;
        }
        chunks.push(Chunk { doc, idx, text, vec, tf, len: toks.len() as u32 });
    }
    let known: HashSet<&String> = m.docs.iter().map(|d| &d.id).collect();
    chunks.retain(|c| known.contains(&c.doc));
    let mut n = Namespace { embedder_id: m.embedder_id, docs: m.docs.into_iter().map(|d| (d.id.clone(), d)).collect(), chunks, df: HashMap::new(), total_len: 0 };
    rebuild_stats(&mut n);
    Ok((m.name, n))
}

/// Default embedder when none is configured.
pub fn default_embedder() -> Arc<dyn TextEmbedder> {
    Arc::new(HashEmbed::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb() -> KnowledgeBase {
        KnowledgeBase::in_memory(default_embedder())
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-kb-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn namespace_names() {
        for ok in ["shared", "user/alice", "session/s_ab12", "skills/user/bob", "a.b/c-d_e"] {
            assert!(valid_namespace(ok), "{ok}");
        }
        for bad in ["", "/x", "x/", "a//b", "A", "../x", "a/../b", "a/b/c/d/e", "a b", &"x".repeat(65), "x/.."] {
            assert!(!valid_namespace(bad), "{bad}");
        }
    }

    #[test]
    fn tokenizer_and_chunker() {
        assert_eq!(tokenize("The Deploying of keys, and the VAULT!"), vec!["deploy", "of", "key", "vault"]);
        let long: String = (0..60).map(|i| format!("Sentence number {i} says something useful. ")).collect();
        let cs = chunk_text(&long, 300, 60);
        assert!(cs.len() > 5 && cs.iter().all(|c| c.len() <= 300), "{:?}", cs.iter().map(|c| c.len()).collect::<Vec<_>>());
        // overlap: each chunk after the first starts with text from the end of the previous one
        let first_words: Vec<&str> = cs[1].split_whitespace().take(2).collect();
        assert!(cs[0].contains(first_words[0]));
        assert!(chunk_text("  \n\n ", 100, 10).is_empty());
        assert!(chunk_text(&"é".repeat(900), 100, 20).iter().all(|c| c.len() <= 100));
        assert_eq!(chunk_text("short", 100, 20), vec!["short"]);
    }

    #[test]
    fn multiple_documents_dedup_and_namespaces_are_isolated() {
        let mut k = kb();
        let (a, new_a) = k.add_document("user/alice", "ops", "ops.md", "The deploy key lives in the vault under prod.", BTreeMap::new()).unwrap();
        let (a2, new_a2) = k.add_document("user/alice", "ops copy", "copy.md", "The deploy key lives in the vault under prod.", BTreeMap::new()).unwrap();
        assert!(new_a && !new_a2 && a == a2, "identical content is stored once");
        k.add_document("user/bob", "bob", "b.md", "Bob's private deploy notes about the vault.", BTreeMap::new()).unwrap();
        k.add_document("shared", "handbook", "h.md", "Shared handbook: rotate every deploy key regularly.", BTreeMap::new()).unwrap();
        assert_eq!(k.stats("user/alice").unwrap().docs, 1);
        let r = k.search(&["user/alice", "shared"], "deploy key vault", 5);
        assert!(!r.is_empty());
        assert!(r.iter().all(|h| h.ns == "user/alice" || h.ns == "shared"), "bob's namespace never leaks into a search that excludes it");
        assert_eq!(r[0].ns, "user/alice");
        assert!(k.search(&["nope"], "deploy", 3).is_empty());
        assert_eq!(k.add_document("Bad Ns", "t", "s", "text", BTreeMap::new()), Err(KbError::BadNamespace));
        assert_eq!(k.add_document("shared", "t", "s", "   ", BTreeMap::new()), Err(KbError::Empty));
        assert!(matches!(k.add_document("shared", "t", "s", &"x".repeat(MAX_DOC_BYTES + 1), BTreeMap::new()), Err(KbError::TooLarge(_))));
    }

    #[test]
    fn hybrid_ranking_beats_either_signal_alone() {
        let mut k = kb();
        let docs = [
            ("kubernetes.md", "Kubernetes pods are scheduled onto nodes by the scheduler. Rolling updates replace pods gradually."),
            ("cooking.md", "Bake the sourdough bread at 230 degrees. Let the dough rest overnight in the fridge."),
            ("wasm.md", "WebAssembly modules run in a sandbox with fuel metering and memory limits for safe execution."),
            ("network.md", "mDNS announces services on the local network so peers can discover each other."),
        ];
        for (t, text) in docs {
            k.add_document("shared", t, t, text, BTreeMap::new()).unwrap();
        }
        let h = k.search(&["shared"], "how are pods scheduled", 2);
        assert_eq!(h[0].title, "kubernetes.md");
        assert!(h[0].bm25 > 0.0 && h[0].vector > 0.0, "both signals contributed");
        assert_eq!(k.search(&["shared"], "sandbox fuel limits", 1)[0].title, "wasm.md");
        assert!(k.search(&["shared"], "zzzzqqqq", 3).is_empty());
        assert!(k.search(&["shared"], "", 3).is_empty());
        assert!(k.search(&["shared"], "pods", 0).is_empty());
    }

    #[test]
    fn mmr_avoids_near_duplicates() {
        let mut k = kb();
        k.add_document("shared", "a", "a", "The vault holds the production deploy key for releases.", BTreeMap::new()).unwrap();
        k.add_document("shared", "b", "b", "The vault holds the production deploy key for all releases.", BTreeMap::new()).unwrap();
        k.add_document("shared", "c", "c", "Rotate the deploy key every ninety days using the vault cli.", BTreeMap::new()).unwrap();
        let h = k.search(&["shared"], "deploy key vault", 2);
        let titles: Vec<&str> = h.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(h.len(), 2);
        assert!(titles.contains(&"c"), "diversified result set: {titles:?}");
    }

    #[test]
    fn namespace_weights_boost_results() {
        let mut k = kb();
        k.add_document("shared", "s", "s", "Quarterly budget planning notes.", BTreeMap::new()).unwrap();
        k.add_document("session/s_1", "m", "m", "Quarterly budget discussion from earlier in this chat.", BTreeMap::new()).unwrap();
        let plain = k.search_weighted(&[("shared", 1.0), ("session/s_1", 1.0)], "quarterly budget", 2);
        let boosted = k.search_weighted(&[("shared", 1.0), ("session/s_1", 3.0)], "quarterly budget", 2);
        assert_eq!(boosted[0].ns, "session/s_1");
        assert_eq!(plain.len(), 2);
    }

    #[test]
    fn remove_and_drop() {
        let mut k = kb();
        let (id, _) = k.add_document("shared", "t", "s", "alpha beta gamma delta", BTreeMap::new()).unwrap();
        k.add_document("shared", "t2", "s2", "epsilon zeta eta theta", BTreeMap::new()).unwrap();
        k.remove_document("shared", &id).unwrap();
        assert!(k.search(&["shared"], "alpha", 3).is_empty());
        assert_eq!(k.search(&["shared"], "epsilon", 3).len(), 1);
        assert_eq!(k.remove_document("shared", &id), Err(KbError::NoSuchDoc));
        k.drop_namespace("shared").unwrap();
        assert_eq!(k.drop_namespace("shared"), Err(KbError::NoSuchNamespace));
        assert!(k.stats("shared").is_none());
    }

    #[test]
    fn persists_and_reloads_exactly() {
        let d = tmp("persist");
        {
            let mut k = KnowledgeBase::open(&d, default_embedder());
            k.add_document("user/alice", "notes", "n.md", "Persistent memory about the zebra project.", BTreeMap::new()).unwrap();
            k.add_document("skills/shared", "sk", "sk", "Skill text about summarising meetings.", BTreeMap::new()).unwrap();
        }
        let k = KnowledgeBase::open(&d, default_embedder());
        assert!(k.load_errors.is_empty(), "{:?}", k.load_errors);
        assert_eq!(k.namespaces(), vec!["skills/shared", "user/alice"]);
        let h = k.search(&["user/alice"], "zebra project", 1);
        assert_eq!(h[0].title, "notes");
        assert!(h[0].vector > 0.0, "vectors survive a restart");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn corrupt_files_are_reported_not_fatal() {
        let d = tmp("corrupt");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("bad.afkb"), b"AFKB1garbage").unwrap();
        std::fs::write(d.join("short.afkb"), b"AF").unwrap();
        let mut ok = KnowledgeBase::open(&d, default_embedder());
        assert_eq!(ok.load_errors.len(), 2);
        ok.add_document("shared", "t", "s", "still works after bad files", BTreeMap::new()).unwrap();
        // truncating a valid file at every length must never panic
        let good = std::fs::read(d.join("shared.afkb")).unwrap();
        for cut in (0..good.len()).step_by(7) {
            let _ = decode_namespace(&good[..cut]);
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn ingest_dir_is_idempotent_and_safe() {
        let d = tmp("ingest");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), "alpha beta gamma").unwrap();
        std::fs::write(d.join("sub/b.md"), "delta epsilon").unwrap();
        std::fs::write(d.join("x.bin"), "ignored").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", d.join("link.txt")).unwrap();
        let mut k = kb();
        assert_eq!(k.ingest_dir("user/alice", &d), (2, 2));
        assert_eq!(k.ingest_dir("user/alice", &d), (0, 2), "second pass adds nothing");
        assert_eq!(k.search(&["user/alice"], "delta", 1)[0].source, "sub/b.md");
        let _ = std::fs::remove_dir_all(d);
    }

    struct Flaky(std::sync::atomic::AtomicBool);
    impl TextEmbedder for Flaky {
        fn id(&self) -> String {
            "flaky".into()
        }
        fn embed(&self, t: &[String]) -> Result<Vec<Vec<f32>>, String> {
            if self.0.load(std::sync::atomic::Ordering::SeqCst) {
                Err("service down".into())
            } else {
                Ok(t.iter().map(|_| vec![1.0, 0.0, 0.0]).collect())
            }
        }
    }

    #[test]
    fn works_without_embeddings_and_reindex_fills_them_in() {
        let down = std::sync::atomic::AtomicBool::new(true);
        let e = Arc::new(Flaky(down));
        let mut k = KnowledgeBase::in_memory(e.clone());
        k.add_document("shared", "t", "s", "keyword searchable text about zebras", BTreeMap::new()).unwrap();
        assert_eq!(k.stats("shared").unwrap().unembedded, 1);
        let h = k.search(&["shared"], "zebras", 1);
        assert_eq!((h.len(), h[0].vector), (1, 0.0), "keyword search still works while embeddings are down");
        assert!(k.reindex("shared").is_err());
        e.0.store(false, std::sync::atomic::Ordering::SeqCst);
        k.reindex("shared").unwrap();
        assert_eq!(k.stats("shared").unwrap().unembedded, 0);
    }

    struct Named(String);
    impl TextEmbedder for Named {
        fn id(&self) -> String {
            self.0.clone()
        }
        fn embed(&self, t: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Ok(t.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    #[test]
    fn documents_embedded_by_another_model_are_found_and_brought_up_to_date_in_one_go() {
        let d = tmp("stale");
        let mut k = KnowledgeBase::open(&d, Arc::new(Named("old".into())));
        k.add_document("shared", "t", "s", "alpha beta", BTreeMap::new()).unwrap();
        k.add_document("user/ann", "t2", "s2", "gamma delta", BTreeMap::new()).unwrap();
        assert!(k.stale_namespaces().is_empty());
        // the person switches to another embedding model
        let mut k = KnowledgeBase::open(&d, Arc::new(Named("new".into())));
        let mut stale = k.stale_namespaces();
        stale.sort();
        assert_eq!(stale, vec!["shared".to_string(), "user/ann".to_string()]);
        assert_eq!(k.reindex_stale(), (2, None));
        assert!(k.stale_namespaces().is_empty());
        // and it stays done after a restart
        let k2 = KnowledgeBase::open(&d, Arc::new(Named("new".into())));
        assert!(k2.stale_namespaces().is_empty());
        // a namespace with a chunk that never got its vector is stale too, and a broken embedder is reported but does not stop the rest
        let down = std::sync::atomic::AtomicBool::new(true);
        let mut k3 = KnowledgeBase::in_memory(Arc::new(Flaky(down)));
        k3.add_document("shared", "t", "s", "text", BTreeMap::new()).unwrap();
        assert_eq!(k3.stale_namespaces(), vec!["shared".to_string()]);
        let (done, err) = k3.reindex_stale();
        assert_eq!(done, 0);
        assert!(err.unwrap().contains("shared"));
        let _ = std::fs::remove_dir_all(d);
    }
}
