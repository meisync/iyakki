//! The RAG tile on a fake port: no knowledge base, no model, no audit log, no threads. The fake answers at once on the channel it is given,
//! or keeps the sender when a test wants a search that is still running (no clock, no sleeps; `tick` is a plain number).

use std::cell::RefCell;
use std::sync::mpsc::Sender;

use iyakki_tile_api::rag::{DocRow, HitRow, PassageRow, RunView, SearchOut};
use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::ui::{Event, Key, R};

use super::*;

#[derive(Default)]
struct Fake {
    docs: RefCell<Vec<DocRow>>,
    out: RefCell<Option<SearchOut>>,
    run: RefCell<Option<RunView>>,
    kept: RefCell<Vec<RunView>>,
    /// Keep the sender instead of answering.
    hold: RefCell<bool>,
    held_search: RefCell<Option<Sender<SearchOut>>>,
    calls: RefCell<Vec<String>>,
    copied: RefCell<Vec<String>>,
    manage: bool,
    externals: usize,
    delete_fails: bool,
}

const ME: &str = "user/tester";

impl Fake {
    fn doc(&self, ns: &str, title: &str, source: &str) {
        let n = self.docs.borrow().len();
        self.docs.borrow_mut().push(DocRow { ns: ns.into(), id: format!("d{n}"), title: title.into(), source: source.into(), chunks: 2 });
    }
    fn note(&self, s: &str) {
        self.calls.borrow_mut().push(s.to_string());
    }
}

fn hit(source: &str, ns: &str) -> HitRow {
    HitRow { ns: ns.into(), doc_id: "d".into(), title: source.into(), source: source.into(), text: format!("text of {source}"), score: 1.0, bm25: 2.5, vector: 0.5 }
}

fn run(agentic: bool, answer: &str) -> RunView {
    RunView {
        run_id: "r1".into(),
        agentic,
        summary: if agentic { "Agentic · 2 of 12 searches".into() } else { "Plain search · no model".into() },
        searches: 2,
        citations: 1,
        stop: "enough evidence".into(),
        lines: (0..40).map(|i| format!("line {i}")).collect(),
        answer: answer.into(),
        passages: vec![PassageRow { id: "shared:ops#0".into(), collection: "shared".into(), source: "ops.md".into(), title: "ops".into(), text: "the key is in the vault".into(), score: 0.9, backend: "local".into() }],
        warnings: vec!["backend slow".into()],
    }
}

impl RagPort for Fake {
    fn namespaces(&self) -> Vec<String> {
        vec![ME.into(), "shared".into()]
    }
    fn docs_folders(&self) -> String {
        "docs/tester  and  docs/shared".into()
    }
    fn external_backends(&self) -> usize {
        self.externals
    }
    fn index(&self, done: Sender<String>) {
        self.note("index");
        let _ = done.send(format!("{} documents indexed", self.docs.borrow().len()));
    }
    fn docs(&self) -> Vec<DocRow> {
        self.docs.borrow().clone()
    }
    fn can_delete(&self, d: &DocRow) -> bool {
        d.ns == ME || (d.ns == "shared" && self.manage)
    }
    fn delete_doc(&self, d: &DocRow) -> Result<(), String> {
        if self.delete_fails {
            return Err("disk full".into());
        }
        self.note(&format!("delete {}", d.source));
        self.docs.borrow_mut().retain(|x| x.id != d.id);
        Ok(())
    }
    fn search(&self, query: &str, ask: bool, done: Sender<SearchOut>) {
        self.note(&format!("{} {query}", if ask { "ask" } else { "search" }));
        if *self.hold.borrow() {
            *self.held_search.borrow_mut() = Some(done);
            return;
        }
        let _ = done.send(self.out.borrow().clone().unwrap_or_else(|| Err("No matching passages.".into())));
    }
    fn research(&self, query: &str, done: Sender<RunView>) {
        self.note(&format!("research {query}"));
        let _ = done.send(self.run.borrow().clone().expect("a run was set up"));
    }
    fn runs(&self) -> Vec<RunView> {
        self.kept.borrow().clone()
    }
    fn copy(&self, text: &str) {
        self.copied.borrow_mut().push(text.to_string());
    }
}

fn typed(t: &mut RagTile, p: &Fake, s: &str) {
    for c in s.chars() {
        t.handle(&Event::Key(Key::Char(c)), p);
    }
}

fn settled(t: &mut RagTile, p: &Fake) {
    t.handle(&Event::Tick, p);
    assert!(!t.is_waiting(), "the fake answers at once");
}

fn opened(p: &Fake) -> RagTile {
    let mut t = RagTile::new(p);
    settled(&mut t, p);
    t
}

fn frame(t: &mut RagTile, p: &Fake, w: u32, h: u32, s: f32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    t.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), s, p, 0);
    g
}

#[test]
fn opening_indexes_the_folders_and_says_how_many_documents() {
    let p = Fake::default();
    p.doc("shared", "Ops", "ops.md");
    p.doc(ME, "Notes", "notes.txt");
    let mut t = RagTile::new(&p);
    assert!(t.is_waiting(), "indexing runs until its sentence arrives");
    t.handle(&Event::Tick, &p);
    assert!(!t.is_waiting());
    assert_eq!(t.info, "2 documents indexed");
    assert_eq!(p.calls.borrow().as_slice(), ["index"]);
    assert_eq!((t.title(), t.accent()), ("RAG", ACCENT));
}

#[test]
fn a_search_shows_the_hits_and_a_click_copies_the_text() {
    let p = Fake::default();
    *p.out.borrow_mut() = Some(Ok((None, vec![hit("ops.md", "shared"), hit("wiki", "ext:corp")], vec!["down: corp".into()])));
    let mut t = opened(&p);
    typed(&mut t, &p, "deploy key");
    assert_eq!(t.handle(&Event::Key(Key::Enter), &p), Answer::Stay);
    assert!(t.is_waiting(), "the search is running");
    settled(&mut t, &p);
    assert_eq!(p.calls.borrow().last().unwrap(), "search deploy key");
    assert_eq!(t.results().len(), 2);
    assert!(t.answer().is_none());
    assert_eq!(t.warnings(), ["down: corp"]);
    let g = frame(&mut t, &p, 900, 600, 1.0);
    assert!(g.hits.iter().any(|h| h.ev == Event::Row(1)), "a row per result");
    t.handle(&Event::Row(1), &p);
    assert_eq!(p.copied.borrow().as_slice(), ["text of wiki"]);
    t.handle(&Event::Row(9), &p);
    assert_eq!(p.copied.borrow().len(), 1, "a row that is not there copies nothing");
}

#[test]
fn a_question_is_ignored_while_empty_or_while_one_is_still_running() {
    let p = Fake::default();
    *p.hold.borrow_mut() = true;
    let mut t = opened(&p);
    t.handle(&Event::Key(Key::Enter), &p);
    assert!(p.calls.borrow().iter().all(|c| c == "index"), "an empty box asks nothing");
    typed(&mut t, &p, "first");
    t.handle(&Event::Key(Key::Enter), &p);
    t.handle(&Event::Key(Key::Enter), &p);
    assert_eq!(p.calls.borrow().iter().filter(|c| c.starts_with("search")).count(), 1, "one at a time");
    assert!(t.is_waiting());
    // the answer arrives later
    let tx = p.held_search.borrow_mut().take().unwrap();
    tx.send(Ok((None, vec![hit("a", "shared")], vec![]))).unwrap();
    t.handle(&Event::Tick, &p);
    assert!(!t.is_waiting() && t.results().len() == 1);
}

#[test]
fn ask_mode_without_the_research_loop_shows_the_answer_and_a_failure_in_plain_words() {
    let p = Fake::default();
    *p.out.borrow_mut() = Some(Ok((Some("It is in the vault [1].".into()), vec![hit("ops.md", "shared")], vec![])));
    let mut t = opened(&p);
    t.handle(&Event::Key(Key::Tab), &p);
    t.handle(&Event::Key(Key::Ctrl('g')), &p);
    assert!(!t.is_agentic());
    typed(&mut t, &p, "where");
    t.handle(&Event::Key(Key::Enter), &p);
    settled(&mut t, &p);
    assert_eq!(p.calls.borrow().last().unwrap(), "ask where");
    assert_eq!(t.answer(), Some("It is in the vault [1]."));
    // a failure clears the rows and shows the sentence
    *p.out.borrow_mut() = Some(Err("The model did not answer.".into()));
    t.handle(&Event::Key(Key::Enter), &p);
    settled(&mut t, &p);
    assert!(t.results().is_empty() && t.answer().is_none());
    assert_eq!(t.error.as_deref(), Some("The model did not answer."));
    let g = frame(&mut t, &p, 900, 600, 1.0);
    assert!(g.hits.iter().any(|h| h.label == "Ask"), "the button names the mode");
}

#[test]
fn the_agentic_switch_runs_the_research_loop_and_the_evidence_page_shows_the_run() {
    let p = Fake::default();
    *p.run.borrow_mut() = Some(run(true, "The key is in the vault [1]."));
    let mut t = opened(&p);
    assert!(t.is_agentic(), "agentic is on by default in ask mode");
    t.handle(&Event::Key(Key::Tab), &p);
    typed(&mut t, &p, "where is the key");
    t.handle(&Event::Key(Key::Enter), &p);
    assert!(t.is_waiting());
    settled(&mut t, &p);
    assert_eq!(p.calls.borrow().last().unwrap(), "research where is the key");
    assert_eq!(t.answer(), Some("The key is in the vault [1]."));
    assert_eq!(t.ledger().unwrap().citations, 1);
    assert_eq!(t.hit_tags, ["local · shared:ops#0"]);
    assert_eq!(t.results()[0].ns, "shared");
    assert_eq!(t.warnings(), ["backend slow"]);
    // the evidence page: Ctrl+E opens, scrolls inside its bounds, Esc closes
    t.handle(&Event::Key(Key::Ctrl('e')), &p);
    assert!(t.ledger_page_open());
    let g = frame(&mut t, &p, 1280, 720, 1.0);
    for label in ["Back", "Older", "Newer", "Up", "Down"] {
        assert!(g.hits.iter().any(|h| h.label == label), "{label}");
    }
    assert!(t.lmax > 0, "40 lines do not fit one page");
    t.handle(&Event::Scroll(-500), &p);
    frame(&mut t, &p, 1280, 720, 1.0);
    assert_eq!(t.lscroll, t.lmax, "the page stops at its end");
    t.handle(&Event::Scroll(500), &p);
    assert_eq!(t.lscroll, 0);
    t.handle(&Event::Key(Key::Esc), &p);
    assert!(!t.ledger_page_open());
    // switching it off makes the next question a plain search and clears the run
    t.handle(&Event::Key(Key::Ctrl('g')), &p);
    assert!(!t.is_agentic());
    *p.out.borrow_mut() = Some(Ok((Some("a".into()), vec![hit("x", "shared")], vec![])));
    t.handle(&Event::Key(Key::Enter), &p);
    settled(&mut t, &p);
    assert!(t.ledger().is_none() && t.hit_tags.is_empty());
}

#[test]
fn a_plain_run_is_drawn_in_the_warning_colour_and_says_so() {
    let p = Fake::default();
    *p.run.borrow_mut() = Some(run(false, "No model is set up."));
    let mut t = opened(&p);
    t.handle(&Event::Key(Key::Tab), &p);
    typed(&mut t, &p, "x");
    t.handle(&Event::Key(Key::Enter), &p);
    settled(&mut t, &p);
    assert!(!t.ledger().unwrap().agentic && t.ledger().unwrap().summary.contains("no model"));
    frame(&mut t, &p, 900, 600, 1.0);
}

#[test]
fn the_evidence_page_lists_kept_runs_once_and_without_a_run_says_so() {
    let p = Fake::default();
    let mut t = opened(&p);
    t.handle(&Event::Tab(63), &p);
    assert!(!t.ledger_page_open());
    assert!(t.error.as_deref().unwrap().contains("No research has been run yet"));
    // two kept runs (one of them is the chat's), the one on screen is not repeated
    let mut old = run(true, "old");
    old.run_id = "r0".into();
    *p.kept.borrow_mut() = vec![run(true, "new"), old];
    *p.run.borrow_mut() = Some(run(true, "new"));
    t.handle(&Event::Key(Key::Tab), &p);
    typed(&mut t, &p, "q");
    t.handle(&Event::Key(Key::Enter), &p);
    settled(&mut t, &p);
    t.handle(&Event::Key(Key::Ctrl('e')), &p);
    assert_eq!(t.runs.len(), 2);
    t.handle(&Event::Key(Key::Right), &p);
    assert_eq!(t.lrun, 1);
    t.handle(&Event::Key(Key::Right), &p);
    assert_eq!(t.lrun, 1, "no run beyond the oldest");
    t.handle(&Event::Key(Key::Left), &p);
    assert_eq!(t.lrun, 0);
}

#[test]
fn the_documents_list_asks_before_it_deletes_and_only_what_the_person_may() {
    let p = Fake::default();
    p.doc("shared", "Sample guide", "sample-guide.md");
    p.doc(ME, "Notes", "notes.txt");
    let mut t = opened(&p);
    t.handle(&Event::Key(Key::Ctrl('o')), &p);
    assert_eq!(t.docs().len(), 2);
    assert_eq!(t.docs()[0].ns, "shared", "shared first");
    assert!(t.docs()[0].is_sample() && !t.docs()[1].is_sample());
    // a shared document is not theirs to delete
    t.handle(&Event::Row(0), &p);
    t.handle(&Event::Key(Key::Char('d')), &p);
    assert!(t.confirm.is_none() && t.status_text().unwrap().contains("administrator"));
    // their own: it asks, anything but y keeps it, y removes it
    t.handle(&Event::Row(1), &p);
    let g = frame(&mut t, &p, 1100, 650, 1.0);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(61)), "a Delete button on the chosen document");
    t.handle(&Event::Key(Key::Char('d')), &p);
    assert!(t.confirm.is_some());
    frame(&mut t, &p, 1100, 650, 1.0);
    t.handle(&Event::Key(Key::Char('n')), &p);
    assert_eq!(t.docs().len(), 2);
    t.handle(&Event::Key(Key::Char('d')), &p);
    t.handle(&Event::Key(Key::Char('y')), &p);
    assert_eq!(t.docs().len(), 1);
    assert!(p.calls.borrow().contains(&"delete notes.txt".to_string()));
    assert_eq!(t.status_text(), Some("Deleted Notes."));
    t.handle(&Event::Key(Key::Esc), &p);
    assert!(!t.docs_open);
}

#[test]
fn an_administrator_may_delete_a_shared_document_and_a_failure_is_shown() {
    let mut p = Fake::default();
    p.manage = true;
    p.doc("shared", "Policy", "policy.md");
    let mut t = opened(&p);
    t.handle(&Event::Tab(60), &p);
    t.handle(&Event::Tab(61), &p);
    assert!(t.confirm.is_some());
    t.handle(&Event::Key(Key::Enter), &p);
    assert!(t.docs().is_empty());
    let mut q = Fake::default();
    q.delete_fails = true;
    q.doc(ME, "Notes", "notes.txt");
    let mut t = opened(&q);
    t.handle(&Event::Tab(60), &q);
    t.handle(&Event::Key(Key::Delete), &q);
    t.handle(&Event::Key(Key::Char('y')), &q);
    assert_eq!(t.docs().len(), 1, "it stays when the port could not remove it");
    assert_eq!(t.status_text(), Some("Could not remove it: disk full"));
}

#[test]
fn the_empty_page_names_the_folders_and_the_external_backends_are_counted() {
    let mut p = Fake::default();
    p.externals = 2;
    let mut t = opened(&p);
    let g = frame(&mut t, &p, 900, 600, 1.0);
    assert!(g.hits.iter().any(|h| h.label == "Documents"));
    assert_eq!(p.docs_folders(), "docs/tester  and  docs/shared");
    // Esc on the search page leaves the tile
    assert_eq!(t.handle(&Event::Key(Key::Esc), &p), Answer::Back);
}

#[test]
fn every_page_fits_the_four_sizes() {
    let p = Fake::default();
    p.doc("shared", "Ops", "ops.md");
    *p.run.borrow_mut() = Some(run(true, "answer"));
    let mut t = opened(&p);
    t.handle(&Event::Key(Key::Tab), &p);
    typed(&mut t, &p, "q");
    t.handle(&Event::Key(Key::Enter), &p);
    settled(&mut t, &p);
    for (w, h, s) in [(1280u32, 720u32, 1.0f32), (800, 480, 1.0), (480, 800, 1.0), (1920, 1080, 2.0)] {
        let area = R::new(0.0, 46.0, w as f32, h as f32 - 46.0);
        let g = frame(&mut t, &p, w, h, s);
        for hit in &g.hits {
            assert!(hit.rect.x >= area.x - 0.5 && hit.rect.x + hit.rect.w <= area.x + area.w + 0.5, "{w}x{h}: {} sticks out", hit.label);
        }
        t.handle(&Event::Key(Key::Ctrl('e')), &p);
        frame(&mut t, &p, w, h, s);
        t.handle(&Event::Key(Key::Esc), &p);
        t.handle(&Event::Key(Key::Ctrl('o')), &p);
        frame(&mut t, &p, w, h, s);
        t.handle(&Event::Key(Key::Esc), &p);
    }
}
