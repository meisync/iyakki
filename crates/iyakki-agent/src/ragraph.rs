//! Graph RAG (F-RAG-15): the people, places, products and terms that documents mention, and the links between them, used to find passages
//! the question's own words do not reach.
//!
//! * [`extract`] takes entities out of text with plain rules (no model): runs of capitalised words (joined by of, de, van, von, la...),
//!   all-capital acronyms and words with digits inside. A capitalised single word at the start of a sentence is not taken on its own, unless the
//!   same word stands alone with a capital elsewhere in the text (mid-sentence), which shows it is a name and not just a capital letter.
//! * [`Graph`] holds, per entity, where it is mentioned, and per pair of entities how many sentences hold both (the link).
//! * [`Graph::expand`] finds the entities the question names (any 1 to 3 words of it that match an entity, in any letter case), follows
//!   their strongest links one step, and returns passages that mention the linked entities but are not in the search result yet, each
//!   with the link that led to it ("Alice Wong <-> Project Orion").
//!
//! It is switched on by the person (`<data>/rag-graph.json`, off by default). It only adds passages after the search's own result, never
//! before it and never in place of it, and every added passage says it came by a link.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::kb::KnowledgeBase;

/// Most entities kept per collection, most links kept per entity.
pub const MAX_ENTITIES: usize = 20_000;
pub const MAX_LINKS: usize = 60;

const CONNECTORS: [&str; 9] = ["of", "de", "van", "von", "la", "le", "del", "da", "the"];
const NOT_ENTITIES: [&str; 28] = ["I", "The", "A", "An", "And", "But", "Or", "If", "In", "On", "At", "To", "It", "Its", "He", "She", "We", "They", "This", "That", "These", "Those", "There", "When", "What", "Who", "How", "Why"];

/// Entities in a text, as written (a repeat is listed once, in the order first seen).
pub fn extract(text: &str) -> Vec<String> {
    let mut known = HashSet::new();
    names_seen_inside(text, &mut known);
    let mut out: Vec<String> = Vec::new();
    for sentence in sentences(text) {
        for e in extract_sentence(&sentence, &known) {
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    out
}

/// The capitalised words that stand alone inside a sentence (not first, with no capitalised word next to them): proof that the word is a name.
fn names_seen_inside(text: &str, known: &mut HashSet<String>) {
    for sentence in sentences(text) {
        let w = words_of(&sentence);
        for i in 1..w.len() {
            let alone = !w.get(i - 1).is_some_and(|p| capitalised(p) || acronym(p)) && !w.get(i + 1).is_some_and(|n| capitalised(n) || acronym(n));
            if capitalised(w[i]) && alone && !CONNECTORS.contains(&w[i - 1]) {
                known.insert(w[i].to_string());
            }
        }
    }
}

fn sentences(text: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        cur.push(*c);
        let end = matches!(c, '.' | '!' | '?' | '\n') && chars.get(i + 1).is_none_or(|n| n.is_whitespace());
        if end {
            if !cur.trim().is_empty() {
                v.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        v.push(cur);
    }
    v
}

fn words_of(s: &str) -> Vec<&str> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '-' || c == '_')).filter(|w| !w.is_empty()).collect()
}

fn capitalised(w: &str) -> bool {
    let mut c = w.chars();
    c.next().is_some_and(|f| f.is_uppercase()) && w.chars().any(|x| x.is_lowercase() || x.is_ascii_digit()) && !NOT_ENTITIES.contains(&w)
}

fn acronym(w: &str) -> bool {
    (2..=6).contains(&w.chars().count()) && w.chars().all(|c| c.is_uppercase() || c.is_ascii_digit()) && w.chars().any(|c| c.is_uppercase()) && !NOT_ENTITIES.contains(&w)
}

fn extract_sentence(s: &str, known: &HashSet<String>) -> Vec<String> {
    let w = words_of(s);
    let mut out = Vec::new();
    let mut i = 0;
    while i < w.len() {
        let first = i == 0;
        if capitalised(w[i]) {
            // a run: capitalised words, with a connector allowed between two of them
            let mut j = i + 1;
            while j < w.len() {
                if capitalised(w[j]) || acronym(w[j]) {
                    j += 1;
                } else if CONNECTORS.contains(&w[j]) && w.get(j + 1).is_some_and(|n| capitalised(n)) {
                    j += 2;
                } else {
                    break;
                }
            }
            let run = &w[i..j];
            // a lone capitalised word at the start of a sentence is just a capital letter, unless it looks like a name (digits or inner capitals)
            let lone_initial = first && run.len() == 1 && !run[0].chars().skip(1).any(|c| c.is_uppercase() || c.is_ascii_digit()) && !known.contains(run[0]);
            if !lone_initial {
                out.push(run.join(" "));
            }
            i = j;
        } else if acronym(w[i]) {
            out.push(w[i].to_string());
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

fn key(e: &str) -> String {
    e.to_lowercase()
}

#[derive(Clone, Debug, Default)]
struct Node {
    name: String,
    /// (document id, passage index)
    at: BTreeSet<(String, u32)>,
}

/// What a collection's documents say about who and what is linked to whom.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    nodes: HashMap<String, Node>,
    links: HashMap<String, BTreeMap<String, u32>>,
    /// Where each passage can be read: document id -> (title, source).
    docs: BTreeMap<String, (String, String)>,
    ns: String,
}

/// A passage found through a link.
#[derive(Clone, Debug, PartialEq)]
pub struct Linked {
    pub ns: String,
    pub doc_id: String,
    pub title: String,
    pub source: String,
    pub chunk_idx: u32,
    pub text: String,
    /// "Alice Wong <-> Project Orion".
    pub via: String,
    pub score: f32,
}

impl Graph {
    /// Read every passage of the collection `ns` of the knowledge base.
    pub fn build(kb: &KnowledgeBase, ns: &str) -> Graph {
        let mut g = Graph { ns: ns.to_string(), ..Graph::default() };
        // first the names the whole collection shows by standing alone inside a sentence, so a sentence that starts with one still counts it
        let mut known = HashSet::new();
        let docs = kb.docs(ns);
        for d in &docs {
            for (_, text) in kb.doc_chunks(ns, &d.id) {
                names_seen_inside(&text, &mut known);
            }
        }
        for d in docs {
            g.docs.insert(d.id.clone(), (d.title.clone(), d.source.clone()));
            for (idx, text) in kb.doc_chunks(ns, &d.id) {
                g.add_passage(&d.id, idx, &text, &known);
            }
        }
        g.trim();
        g
    }

    fn add_passage(&mut self, doc: &str, idx: u32, text: &str, known: &HashSet<String>) {
        for sentence in sentences(text) {
            let ents = extract_sentence(&sentence, known);
            let keys: Vec<String> = ents.iter().map(|e| key(e)).collect();
            for (e, k) in ents.iter().zip(&keys) {
                if self.nodes.len() >= MAX_ENTITIES && !self.nodes.contains_key(k) {
                    continue;
                }
                let n = self.nodes.entry(k.clone()).or_insert_with(|| Node { name: e.clone(), at: BTreeSet::new() });
                n.at.insert((doc.to_string(), idx));
            }
            for a in &keys {
                for b in &keys {
                    if a != b && self.nodes.contains_key(a) && self.nodes.contains_key(b) {
                        *self.links.entry(a.clone()).or_default().entry(b.clone()).or_default() += 1;
                    }
                }
            }
        }
    }

    /// Keep only the strongest links of each entity.
    fn trim(&mut self) {
        for m in self.links.values_mut() {
            if m.len() > MAX_LINKS {
                let mut v: Vec<(String, u32)> = std::mem::take(m).into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                v.truncate(MAX_LINKS);
                *m = v.into_iter().collect();
            }
        }
    }

    pub fn entities(&self) -> usize {
        self.nodes.len()
    }

    pub fn link_count(&self) -> usize {
        self.links.values().map(|m| m.len()).sum::<usize>() / 2
    }

    /// The entities that are mentioned most.
    pub fn top(&self, n: usize) -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = self.nodes.values().map(|x| (x.name.clone(), x.at.len())).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    /// The entities of the graph that a question names: any 1 to 3 words of it that match an entity's name, in any letter case.
    pub fn named_in(&self, question: &str) -> Vec<String> {
        let w: Vec<String> = words_of(question).iter().map(|x| x.to_lowercase()).collect();
        let mut found: Vec<String> = Vec::new();
        for n in (1..=3).rev() {
            for win in w.windows(n) {
                let k = win.join(" ");
                if self.nodes.contains_key(&k) && !found.contains(&k) && !found.iter().any(|f| f.contains(&k)) {
                    found.push(k);
                }
            }
        }
        found
    }

    /// An entity's name as written, its strongest links and the documents that mention it.
    pub fn related(&self, entity: &str) -> Option<(String, Vec<(String, u32)>, Vec<(String, String)>)> {
        let k = key(entity.trim());
        let n = self.nodes.get(&k)?;
        let mut links: Vec<(String, u32)> = self.links.get(&k).map(|m| m.iter().filter_map(|(o, w)| self.nodes.get(o).map(|x| (x.name.clone(), *w))).collect()).unwrap_or_default();
        links.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        links.truncate(10);
        let mut docs: Vec<(String, String)> = n.at.iter().map(|(d, _)| d.clone()).collect::<BTreeSet<_>>().into_iter().filter_map(|d| self.docs.get(&d).map(|(t, _)| (d.clone(), t.clone()))).collect();
        docs.truncate(10);
        Some((n.name.clone(), links, docs))
    }

    /// Passages that mention an entity linked to one the question names and are not among `have` (document id, passage index), best
    /// first, at most `limit`. `text_of` reads a passage (the graph keeps no text).
    pub fn expand(&self, question: &str, have: &[(String, u32)], limit: usize, text_of: &dyn Fn(&str, u32) -> Option<String>) -> Vec<Linked> {
        let named = self.named_in(question);
        if named.is_empty() || limit == 0 {
            return vec![];
        }
        let mut cand: BTreeMap<(String, u32), (f32, String)> = BTreeMap::new();
        for q in &named {
            let Some(qn) = self.nodes.get(q) else { continue };
            let mut ls: Vec<(&String, &u32)> = self.links.get(q).map(|m| m.iter().collect()).unwrap_or_default();
            ls.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            for (o, w) in ls.into_iter().filter(|(o, _)| !named.contains(o)).take(5) {
                let Some(on) = self.nodes.get(o) else { continue };
                let via = format!("{} <-> {}", qn.name, on.name);
                for at in &on.at {
                    if have.contains(at) {
                        continue;
                    }
                    // a passage that also names the question's entity is the strongest evidence
                    let both = qn.at.contains(at);
                    let s = *w as f32 * if both { 2.0 } else { 1.0 };
                    let e = cand.entry(at.clone()).or_insert((0.0, via.clone()));
                    if s > e.0 {
                        *e = (s, via.clone());
                    }
                }
            }
        }
        let mut v: Vec<((String, u32), (f32, String))> = cand.into_iter().collect();
        v.sort_by(|a, b| b.1 .0.partial_cmp(&a.1 .0).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        v.into_iter()
            .filter_map(|((doc, idx), (score, via))| {
                let text = text_of(&doc, idx)?;
                let (title, source) = self.docs.get(&doc).cloned().unwrap_or_default();
                Some(Linked { ns: self.ns.clone(), doc_id: doc, title, source, chunk_idx: idx, text, via, score })
            })
            .take(limit)
            .collect()
    }
}

// ------------------------------------------------------------------ the switch and the cache

fn path(data: &Path) -> std::path::PathBuf {
    data.join("rag-graph.json")
}

/// Whether the person switched Graph RAG on (off until they do).
pub fn is_on(data: &Path) -> bool {
    std::fs::read_to_string(path(data)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v.get("on").and_then(|b| b.as_bool())).unwrap_or(false)
}

pub fn set_on(data: &Path, on: bool) -> Result<(), String> {
    iyakki_schema::write_atomic(&path(data), serde_json::json!({"on": on}).to_string().as_bytes(), 0o644)
}

static CACHE: Mutex<Option<HashMap<String, ((usize, usize), Arc<Graph>)>>> = Mutex::new(None);

/// The graph of a collection, built when it was not built yet or when the collection's size changed.
pub fn graph_of(kb: &KnowledgeBase, ns: &str) -> Arc<Graph> {
    let sig = kb.stats(ns).map(|s| (s.docs, s.chunks)).unwrap_or((0, 0));
    let ck = format!("{:p}:{ns}", kb as *const KnowledgeBase);
    {
        let g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((s, gr)) = g.as_ref().and_then(|m| m.get(&ck)) {
            if *s == sig {
                return gr.clone();
            }
        }
    }
    let built = Arc::new(Graph::build(kb, ns));
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(ck, (sig, built.clone()));
    built
}

#[cfg(test)]
#[path = "ragraph_tests.rs"]
mod tests;
