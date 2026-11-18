//! Offline, deterministic evaluation of retrieval: plain hybrid search against the agentic loop on the golden set in
//! `tests/fixtures/rag/` (synthetic documents, questions and the sources that answer them). The model is a script: a grader and a
//! rewriter that know the answer phrases, so the numbers measure the LOOP (plan, grade, rewrite, keep), not a real model. The test
//! prints recall@k and MRR for both and FAILS when the agentic loop is worse than plain search.
//!
//! Run `cargo test -p iyakki-agent --lib -- rageval_tests --nocapture` to see the table.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::agenticrag::{research, Budget, Mode, Request};
use crate::agenticrag_tests::{kind_of, Model};
use crate::kb::{default_embedder, KnowledgeBase};
use crate::ragtools::{Filters, RagAccess};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rag")
}

fn golden() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixtures().join("golden.json")).unwrap()).unwrap()
}

fn access() -> RagAccess {
    let kb = Arc::new(Mutex::new(KnowledgeBase::in_memory(default_embedder())));
    let (added, seen) = kb.lock().unwrap().ingest_dir("user/eval", &fixtures().join("docs"));
    assert!(seen >= 12 && added == seen, "{added} of {seen}");
    RagAccess::new(kb, "eval", None)
}

/// The passages of a grade request: `(index, text)`.
fn passages_in(user: &str) -> Vec<(usize, String)> {
    let body = user.split(crate::rag::DATA_OPEN).nth(1).and_then(|b| b.split(crate::rag::DATA_CLOSE).next()).unwrap_or("");
    body.split("\n\n")
        .filter_map(|b| {
            let b = b.trim();
            let rest = b.strip_prefix('[')?;
            let (n, _) = rest.split_once(']')?;
            Some((n.parse().ok()?, b.to_lowercase()))
        })
        .collect()
}

/// A model that knows, per sub-question, the phrases that answer it and the better queries to try.
fn oracle(g: &Value) -> Model {
    let plans: Vec<(String, Vec<String>, Vec<String>, Vec<String>)> = g["cases"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| {
            let subs: Vec<String> = c["plan"].as_array().unwrap().iter().map(|s| s["q"].as_str().unwrap().to_string()).collect();
            let whole = c["q"].as_str().unwrap().to_string();
            c["plan"].as_array().unwrap().iter().map(move |s| {
                let list = |k: &str| s[k].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect::<Vec<_>>();
                (s["q"].as_str().unwrap().to_string(), list("keys"), list("rewrites"), if s["q"].as_str().unwrap() == whole { subs.clone() } else { vec![] })
            }).collect::<Vec<_>>()
        })
        .collect();
    let whole_plans: Vec<(String, Vec<String>)> = g["cases"].as_array().unwrap().iter().map(|c| (c["q"].as_str().unwrap().to_string(), c["plan"].as_array().unwrap().iter().map(|s| s["q"].as_str().unwrap().to_string()).collect())).collect();
    let used = Mutex::new(std::collections::BTreeMap::<String, usize>::new());
    Model::new(move |kind, user| {
        let question_of = |u: &str| u.rsplit("Question: ").next().unwrap_or("").lines().next().unwrap_or("").trim().to_string();
        match kind {
            "plan" => {
                let subs = whole_plans.iter().find(|p| p.0 == user.trim()).map(|p| p.1.clone()).unwrap_or_else(|| vec![user.trim().to_string()]);
                Ok(serde_json::to_string(&subs).unwrap())
            }
            "grade" => {
                let q = question_of(user);
                let keys = plans.iter().find(|p| p.0 == q).map(|p| p.1.clone()).unwrap_or_default();
                let ps = passages_in(user);
                let obj: Vec<String> = ps.iter().map(|(n, t)| format!("\"{n}\":\"{}\"", if keys.iter().any(|k| t.contains(&k.to_lowercase())) { "relevant" } else { "irrelevant" })).collect();
                Ok(format!("{{{}}}", obj.join(",")))
            }
            "rewrite" => {
                let q = user.lines().next().unwrap_or("").trim_start_matches("Question: ").trim().to_string();
                let mut u = used.lock().unwrap();
                let i = u.entry(q.clone()).or_insert(0);
                let r = plans.iter().find(|p| p.0 == q).and_then(|p| p.2.get(*i).cloned()).unwrap_or_else(|| "no idea".into());
                *i += 1;
                Ok(r)
            }
            _ => Ok("Answer based on the sources [1].".into()),
        }
    })
}

fn source_of(a: &RagAccess, id: &str) -> String {
    // ids are `<collection>:<doc>#<n>`; the file name is the document's title
    let (coll, rest) = id.split_once(':').unwrap();
    let (doc, _) = rest.rsplit_once('#').unwrap();
    let ns = a.to_ns(coll).unwrap();
    let kb = a.visible();
    assert!(kb.iter().any(|v| v.1 == ns));
    a.read(id, 0, false).ok().and_then(|t| t.split('“').nth(1).and_then(|s| s.split('”').next()).map(str::to_string)).unwrap_or_else(|| doc.to_string())
}

#[derive(Default, Clone, Copy)]
struct Score {
    recall: f64,
    rr: f64,
    n: f64,
}

impl Score {
    fn add(&mut self, expect: &[String], ranked: &[String], k: usize) {
        let top: Vec<&String> = ranked.iter().take(k).collect();
        let found = expect.iter().filter(|e| top.contains(e)).count();
        self.recall += found as f64 / expect.len() as f64;
        self.rr += top.iter().position(|s| expect.contains(s)).map_or(0.0, |i| 1.0 / (i as f64 + 1.0));
        self.n += 1.0;
    }
    fn recall(&self) -> f64 {
        self.recall / self.n
    }
    fn mrr(&self) -> f64 {
        self.rr / self.n
    }
}

fn dedup_sources(v: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    v.into_iter().filter(|s| seen.insert(s.clone())).collect()
}

#[test]
fn the_agentic_loop_is_no_worse_than_plain_search_on_the_golden_set() {
    let g = golden();
    let k = g["k"].as_u64().unwrap() as usize;
    let a = access();
    let model = oracle(&g);
    let (mut plain, mut agentic) = (Score::default(), Score::default());
    let mut rows = vec![];
    for c in g["cases"].as_array().unwrap() {
        let q = c["q"].as_str().unwrap();
        let expect: Vec<String> = c["expect"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        // plain: one hybrid search of the whole question, the passages in rank order
        let ps = a.search_with(&[], q, k, &Filters::default()).unwrap().0;
        let plain_src: Vec<String> = dedup_sources(ps.iter().map(|p| p.source.clone()).collect());
        plain.add(&expect, &plain_src, k);
        // agentic: the kept passages in the order the loop ranks them
        let l = research(&a, Some(&model), &Request { question: q, collections: &[], budget: Budget { k, ..Budget::default() } }, &AtomicBool::new(false));
        assert_eq!(l.mode, Mode::Agentic);
        assert!(l.searches <= 12 && l.model_calls <= 24);
        let kept: Vec<String> = dedup_sources(l.ranked_ids().iter().filter_map(|id| l.passage(id)).map(|p| p.source.clone()).collect());
        agentic.add(&expect, &kept, k);
        let _ = source_of;
        rows.push(format!("  {:<62} plain {:?}  agentic {:?}  ({} searches, {})", q.chars().take(60).collect::<String>(), plain_src, kept, l.searches, l.stop.text()));
    }
    let table = format!(
        "RAG evaluation (k={k}, {} questions)\n{}\n  plain search : recall@{k} {:.3}  MRR {:.3}\n  agentic loop : recall@{k} {:.3}  MRR {:.3}\n",
        plain.n as usize,
        rows.join("\n"),
        plain.recall(),
        plain.mrr(),
        agentic.recall(),
        agentic.mrr()
    );
    println!("{table}");
    assert!(agentic.recall() >= plain.recall() && agentic.mrr() >= plain.mrr(), "the agentic loop is WORSE than plain search:\n{table}");
    assert!(agentic.recall() >= 0.9, "the loop should find the sources of this set:\n{table}");
}

#[test]
fn questions_the_sources_cannot_answer_end_in_not_found() {
    let g = golden();
    let a = access();
    let model = oracle(&g);
    for c in g["unanswerable"].as_array().unwrap() {
        let q = c["q"].as_str().unwrap();
        let l = research(&a, Some(&model), &Request { question: q, collections: &[], budget: Budget::default() }, &AtomicBool::new(false));
        assert!(l.answer.starts_with("Not found in the sources"), "{q}: {}", l.answer);
        assert!(l.citations.is_empty() && l.passages.is_empty());
        assert!(l.searches <= 12);
        println!("unanswerable “{q}”: {} ({} searches)", l.stop.text(), l.searches);
    }
}

#[test]
fn without_a_model_the_numbers_are_plain_search_and_the_ledger_says_so() {
    let g = golden();
    let a = access();
    let mut s = Score::default();
    for c in g["cases"].as_array().unwrap() {
        let q = c["q"].as_str().unwrap();
        let expect: Vec<String> = c["expect"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        let l = research(&a, None, &Request { question: q, collections: &[], budget: Budget { k: 3, ..Budget::default() } }, &AtomicBool::new(false));
        assert_eq!((l.mode, l.searches), (Mode::Plain, 1));
        s.add(&expect, &dedup_sources(l.passages.iter().map(|p| p.source.clone()).collect()), 3);
    }
    println!("no-model run: recall@3 {:.3} MRR {:.3} (equal to plain search)", s.recall(), s.mrr());
    let _ = kind_of;
}
