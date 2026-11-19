//! Types moved out of `iyakki_agent::agenticrag` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

pub const MAX_SUBQUESTIONS: usize = 4;

pub const MAX_ROUNDS: usize = 3;

pub const MAX_SEARCHES: usize = 12;

/// The limits of one run. [`Budget::clamped`] holds them under the hard caps above.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub max_subquestions: usize,
    pub max_rounds: usize,
    pub max_searches: usize,
    /// Passages asked for per search.
    pub k: usize,
}

impl Default for Budget {
    fn default() -> Budget {
        Budget { max_subquestions: MAX_SUBQUESTIONS, max_rounds: MAX_ROUNDS, max_searches: MAX_SEARCHES, k: 5 }
    }
}

impl Budget {
    pub fn clamped(self) -> Budget {
        Budget { max_subquestions: self.max_subquestions.clamp(1, MAX_SUBQUESTIONS), max_rounds: self.max_rounds.clamp(1, MAX_ROUNDS), max_searches: self.max_searches.clamp(1, MAX_SEARCHES), k: self.k.clamp(1, 10) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Agentic,
    Plain,
}

pub struct Request<'a> {
    pub question: &'a str,
    pub collections: &'a [String],
    pub budget: Budget,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use std::collections::HashSet;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

pub const LEDGER_VERSION: u32 = 1;

/// One retrieved passage with where it came from. Text, title and source are already redacted of credentials.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RagPassage {
    /// `<collection>:<document>#<chunk>`, the handle `rag_read` takes and citations carry.
    pub id: String,
    pub collection: String,
    pub source: String,
    pub title: String,
    pub text: String,
    pub score: f32,
    /// `local` or `external:<id>`.
    pub backend: String,
    pub provenance: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grade {
    Relevant,
    Partly,
    Irrelevant,
}

impl Grade {
    pub fn label(self) -> &'static str {
        match self {
            Grade::Relevant => "relevant",
            Grade::Partly => "partly",
            Grade::Irrelevant => "irrelevant",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub round: usize,
    pub query: String,
    /// Passages the search returned, and how many of them were new to this question.
    pub found: usize,
    pub new: usize,
    pub grades: Vec<(String, Grade)>,
    /// `model` or `overlap`: who graded.
    pub graded_by: String,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubLedger {
    pub question: String,
    pub attempts: Vec<Attempt>,
    /// Ids of the passages kept for this question (relevant first).
    pub kept: Vec<String>,
    pub sufficient: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stop {
    /// Every question has enough evidence.
    Sufficient,
    /// The search budget is spent.
    SearchBudget,
    /// Each question used its rounds (or had no new query to try).
    RoundsSpent,
    /// Nothing relevant was found.
    NothingFound,
    /// No model: one plain search was done.
    NoModel,
    Cancelled,
    /// A collection was refused or a search failed.
    SearchFailed,
}

impl Stop {
    pub fn text(self) -> &'static str {
        match self {
            Stop::Sufficient => "enough evidence found",
            Stop::SearchBudget => "search budget spent",
            Stop::RoundsSpent => "rounds spent",
            Stop::NothingFound => "nothing relevant found",
            Stop::NoModel => "no model: plain search only",
            Stop::Cancelled => "cancelled",
            Stop::SearchFailed => "search failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub n: usize,
    pub id: String,
    pub source: String,
    pub collection: String,
}

/// What a run did, kept per run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    pub version: u32,
    pub run_id: String,
    pub at: u64,
    pub question: String,
    pub collections: Vec<String>,
    pub budget: Budget,
    pub mode: Mode,
    pub searches: usize,
    pub model_calls: usize,
    pub subs: Vec<SubLedger>,
    /// The passages kept, in the order the answer numbers them.
    pub passages: Vec<RagPassage>,
    pub stop: Stop,
    pub answer: String,
    pub citations: Vec<Citation>,
    pub notes: Vec<String>,
    pub warnings: Vec<String>,
}

impl Ledger {
    pub fn new(question: &str, collections: &[String], budget: Budget, run_id: String) -> Ledger {
        Ledger { version: LEDGER_VERSION, run_id, at: iyakki_clock::now_unix().max(0) as u64, question: question.into(), collections: collections.to_vec(), budget, mode: Mode::Agentic, searches: 0, model_calls: 0, subs: vec![], passages: vec![], stop: Stop::NothingFound, answer: String::new(), citations: vec![], notes: vec![], warnings: vec![] }
    }

    /// The line the tile and the tool result always show: mode, budget used, stop reason.
    pub fn summary_line(&self) -> String {
        format!(
            "{} · {} of {} searches · up to {} rounds and {} questions · {} model calls · stopped: {}",
            if self.mode == Mode::Agentic { "Agentic" } else { "Plain search" },
            self.searches,
            self.budget.max_searches,
            self.budget.max_rounds,
            self.budget.max_subquestions,
            self.model_calls,
            self.stop.text()
        )
    }

    /// Ids of the kept passages in ranking order (each question's, relevant first; no repeats).
    pub fn ranked_ids(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.subs.iter().flat_map(|s| s.kept.iter().cloned()).filter(|i| seen.insert(i.clone())).collect()
    }

    pub fn passage(&self, id: &str) -> Option<&RagPassage> {
        self.passages.iter().find(|p| p.id == id)
    }

    /// The ledger as plain lines (the tile draws these; the tool result carries them).
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![self.summary_line(), format!("Question: {}", self.question)];
        for (i, s) in self.subs.iter().enumerate() {
            v.push(format!("{}. {} ({})", i + 1, s.question, if s.sufficient { "enough evidence" } else { "not enough" }));
            for a in &s.attempts {
                let g = |w: Grade| a.grades.iter().filter(|x| x.1 == w).count();
                v.push(format!("   round {}: “{}” -> {} found, {} new; {} relevant, {} partly, {} irrelevant (graded by {}){}", a.round, a.query, a.found, a.new, g(Grade::Relevant), g(Grade::Partly), g(Grade::Irrelevant), a.graded_by, if a.note.is_empty() { String::new() } else { format!(" - {}", a.note) }));
            }
            if !s.kept.is_empty() {
                v.push(format!("   kept: {}", s.kept.join(", ")));
            }
        }
        for c in &self.citations {
            v.push(format!("[{}] {} - {}", c.n, c.source, c.id));
        }
        for n in self.notes.iter().chain(self.warnings.iter()) {
            v.push(format!("note: {n}"));
        }
        v
    }
}

pub static RUNS: AtomicU64 = AtomicU64::new(1);

pub fn new_run_id() -> String {
    format!("{:x}-{:x}", iyakki_clock::now_unix().max(0), RUNS.fetch_add(1, Ordering::Relaxed))
}

pub fn cap(s: &str, n: usize) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= n {
        t
    } else {
        t.chars().take(n).collect::<String>() + "…"
    }
}
