//! The plain data and the port of the RAG tile (D-S2-N2-01): the documents, the search hits and the research runs as plain mirrors of the
//! types in `iyakki_shared` (`kb::Hit`, `agenticrag::Ledger`), and what the tile asks of the rest of the system. The shell converts between
//! the two. The tile names no `Services`, no session, no settings and no audit log: the real port starts the searches on worker threads
//! (embedding may call a remote model) and sends the answer back on the channel it was given, a fake answers at once.

use std::sync::mpsc::Sender;

/// One indexed document in the Documents list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocRow {
    pub ns: String,
    pub id: String,
    pub title: String,
    pub source: String,
    pub chunks: usize,
}

impl DocRow {
    /// Put there by IYAKKI as an example (its file name starts with `sample-`).
    pub fn is_sample(&self) -> bool {
        self.source.rsplit('/').next().is_some_and(|f| f.starts_with("sample-"))
    }
}

/// One search result (a mirror of `iyakki_shared::kb::Hit`).
#[derive(Clone, Debug, PartialEq)]
pub struct HitRow {
    pub ns: String,
    pub doc_id: String,
    pub title: String,
    pub source: String,
    pub text: String,
    pub score: f32,
    pub bm25: f32,
    pub vector: f32,
}

/// One passage a research run kept (a mirror of `agenticrag::RagPassage`).
#[derive(Clone, Debug, PartialEq)]
pub struct PassageRow {
    pub id: String,
    pub collection: String,
    pub source: String,
    pub title: String,
    pub text: String,
    pub score: f32,
    /// `local` or `external:<id>`.
    pub backend: String,
}

/// What a research run did (a mirror of `agenticrag::Ledger`, with its lines already written out).
#[derive(Clone, Debug, PartialEq)]
pub struct RunView {
    pub run_id: String,
    /// The bounded research loop ran (false: a plain search was all that happened).
    pub agentic: bool,
    /// The one line that is always shown: mode, budget used, stop reason.
    pub summary: String,
    pub searches: usize,
    pub citations: usize,
    /// The stop reason in words.
    pub stop: String,
    /// The ledger as plain lines (the first is the summary, the second the question).
    pub lines: Vec<String>,
    pub answer: String,
    pub passages: Vec<PassageRow>,
    pub warnings: Vec<String>,
}

impl RunView {
    /// The result rows of the run: its kept passages, as search hits.
    pub fn hits(&self) -> Vec<HitRow> {
        self.passages
            .iter()
            .map(|p| HitRow { ns: if let Some(e) = p.backend.strip_prefix("external:") { format!("ext:{e}") } else { p.collection.clone() }, doc_id: p.id.clone(), title: p.title.clone(), source: p.source.clone(), text: p.text.clone(), score: p.score, bm25: 0.0, vector: 0.0 })
            .collect()
    }
    /// What each result row says about where it came from.
    pub fn tags(&self) -> Vec<String> {
        self.passages.iter().map(|p| format!("{} · {}", p.backend, p.id)).collect()
    }
}

/// A finished search: the model's answer (ask mode), the hits and the warnings of the backends, or the sentence to show.
pub type SearchOut = Result<(Option<String>, Vec<HitRow>, Vec<String>), String>;

pub trait RagPort {
    /// The namespaces the signed-in person searches (their own and the shared one).
    fn namespaces(&self) -> Vec<String>;
    /// The folders the documents come from, for the empty page.
    fn docs_folders(&self) -> String;
    /// How many external RAG backends Settings has switched on.
    fn external_backends(&self) -> usize;
    /// Index the documents folders in the background (unchanged files are skipped); the sentence to show goes to `done`.
    fn index(&self, done: Sender<String>);
    /// The documents of the person's own namespace and the shared one.
    fn docs(&self) -> Vec<DocRow>;
    /// The person may delete the document: their own, or a shared one when they manage the system.
    fn can_delete(&self, doc: &DocRow) -> bool;
    /// Remove the document from the index and, when it came from a file in the documents folder, delete the file; writes the audit line.
    fn delete_doc(&self, doc: &DocRow) -> Result<(), String>;
    /// Search (and, with `ask`, answer from what was found); runs on a worker thread, the result goes to `done`. Writes the audit line.
    fn search(&self, query: &str, ask: bool, done: Sender<SearchOut>);
    /// The bounded research loop (plan, search, grade, rewrite, cite); the finished run goes to `done`. Writes the audit line.
    fn research(&self, query: &str, done: Sender<RunView>);
    /// The person's kept runs (also the ones the chat made), newest first.
    fn runs(&self) -> Vec<RunView>;
    /// Copy a result's text.
    fn copy(&self, text: &str);
}
