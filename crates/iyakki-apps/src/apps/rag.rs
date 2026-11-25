//! RAG: the tile itself lives in the `iyakki-tile-rag` crate (it does not depend on the agent or the shared crate). This file is the shell's
//! side of it: the adapter that makes the tile an `App` of the shell, over the real port in `rag_port.rs` (the knowledge base, the research
//! kit, the model, the documents folder, the audit log). Search your documents (and any external RAG backends Settings enables), or ask
//! the model a question grounded in what was found; each person searches their own namespace plus the shared one.

use super::rag_port::CxPort;
use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};
use iyakki_tile_rag as tile;
use tile::{Answer, RagPort};

pub use iyakki_tile_api::rag::DocRow;

pub struct RagApp {
    tile: tile::RagTile,
}

impl RagApp {
    pub fn new(cx: &AppCx) -> RagApp {
        RagApp::with_port(&CxPort(cx))
    }

    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn RagPort) -> RagApp {
        RagApp { tile: tile::RagTile::new(port) }
    }
}

impl std::ops::Deref for RagApp {
    type Target = tile::RagTile;
    fn deref(&self) -> &tile::RagTile {
        &self.tile
    }
}

impl std::ops::DerefMut for RagApp {
    fn deref_mut(&mut self) -> &mut tile::RagTile {
        &mut self.tile
    }
}

impl App for RagApp {
    fn title(&self) -> &'static str {
        self.tile.title()
    }
    fn icon(&self) -> Icon {
        self.tile.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.tile.accent()
    }
    fn hint(&self) -> String {
        self.tile.hint()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match self.tile.handle(ev, &CxPort(cx)) {
            Answer::Stay => Flow::Stay,
            Answer::Back => Flow::Back,
        }
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        self.tile.context_items(x, y)
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.tile.render(g, area, s, &CxPort(cx), tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::rgb;
    use crate::ui::Key;
    use crate::apps::tests::{cx_with_backend, services_in, temp_dir, Offline};
    use iyakki_proxy::{Upstream, UpstreamResponse};
    use iyakki_shared::chat::{ChatBackend, Message};
    use iyakki_shared::rag_ext::ExternalRag;
    use iyakki_shared::settings::ExtEntry;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[test]
    fn the_documents_list_shows_samples_and_deletes_a_document_and_its_file_after_asking() {
        let (mut cx, d) = cx_with_backend("ragdocs", iyakki_shared::chat::Unconfigured);
        let docs = d.join("docs");
        std::fs::create_dir_all(docs.join("shared")).unwrap();
        std::fs::create_dir_all(docs.join("tester")).unwrap();
        std::fs::write(docs.join("shared/sample-guide.md"), "# Sample guide\nthe deploy window is Tuesday").unwrap();
        std::fs::write(docs.join("shared/policy.md"), "company policy text about holidays").unwrap();
        std::fs::write(docs.join("tester/notes.txt"), "my own notes about gardening").unwrap();
        cx.cfg.rag_docs = docs.display().to_string();
        let mut app = RagApp::new(&cx);
        settle(&mut app, &cx);
        // the Documents list: shared and mine, the sample marked
        app.handle(&Event::Key(Key::Ctrl('o')), &cx);
        assert_eq!(app.docs().len(), 3, "{:?}", app.docs());
        assert!(app.docs().iter().find(|d| d.source == "sample-guide.md").unwrap().is_sample());
        assert!(!app.docs().iter().find(|d| d.source == "policy.md").unwrap().is_sample());
        let mut g = Gfx::new(1100, 650);
        g.clear(rgb(9, 11, 20));
        app.render(&mut g, R::new(0.0, 46.0, 1100.0, 604.0), 1.0, &cx, 0);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(61)), "a Delete button on the chosen document");
        // delete the sample: it asks, anything but y keeps it, y removes it from the index and deletes its file
        let i = app.docs().iter().position(|d| d.source == "sample-guide.md").unwrap();
        app.handle(&Event::Row(i), &cx);
        app.handle(&Event::Key(Key::Char('d')), &cx);
        assert!(app.confirm.is_some());
        app.render(&mut g, R::new(0.0, 46.0, 1100.0, 604.0), 1.0, &cx, 0);
        app.handle(&Event::Key(Key::Char('n')), &cx);
        assert_eq!(app.docs().len(), 3);
        app.handle(&Event::Key(Key::Char('d')), &cx);
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert_eq!(app.docs().len(), 2);
        assert!(!docs.join("shared/sample-guide.md").exists(), "the file is gone so it is not indexed again");
        let (hits, _) = cx.services.search(&["shared"], "deploy window", 5);
        assert!(hits.iter().all(|h| !h.text.contains("Tuesday")), "it can no longer be found");
        let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
        assert!(log.contains("rag.delete"), "{log}");
        // opening RAG again does not bring it back
        let mut again = RagApp::new(&cx);
        settle(&mut again, &cx);
        again.handle(&Event::Tab(60), &cx);
        assert_eq!(again.docs().len(), 2);
        // my own document can be deleted by me; a shared one only by someone who manages the system
        cx.session.policy.manage_settings = false;
        let mut app = RagApp::new(&cx);
        settle(&mut app, &cx);
        app.handle(&Event::Tab(60), &cx);
        let shared = app.docs().iter().position(|d| d.ns == "shared").unwrap();
        app.handle(&Event::Row(shared), &cx);
        app.handle(&Event::Key(Key::Char('d')), &cx);
        assert!(app.confirm.is_none() && app.status_text().unwrap().contains("administrator"));
        let mine = app.docs().iter().position(|d| d.ns != "shared").unwrap();
        app.handle(&Event::Row(mine), &cx);
        app.handle(&Event::Key(Key::Char('d')), &cx);
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert!(!docs.join("tester/notes.txt").exists());
        // Esc leaves the list for the search page
        app.handle(&Event::Key(Key::Esc), &cx);
        assert!(!app.docs_open);
    }

    fn typed(app: &mut RagApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            app.handle(&Event::Key(Key::Char(c)), cx);
        }
    }

    fn settle(app: &mut RagApp, cx: &AppCx) {
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
            app.handle(&Event::Tick, cx);
            if !app.is_waiting() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("RAG app did not settle");
    }

    #[test]
    fn indexes_folders_then_searches_and_asks() {
        struct Fixed;
        impl ChatBackend for Fixed {
            fn complete(&self, h: &[Message]) -> Result<String, String> {
                assert!(h[1].content.contains("vault"));
                Ok("It is in the vault [1].".into())
            }
        }
        let (mut cx, d) = cx_with_backend("rag1", Fixed);
        let docs = d.join("docs");
        std::fs::create_dir_all(docs.join("shared")).unwrap();
        std::fs::write(docs.join("shared/ops.md"), "The deploy key is stored in the vault under prod.").unwrap();
        cx.cfg.rag_docs = docs.display().to_string();
        cx.services = services_in(&d, Arc::new(Offline));
        let mut app = RagApp::new(&cx);
        settle(&mut app, &cx);
        assert!(app.info.contains("1 documents indexed"), "{}", app.info);
        typed(&mut app, &cx, "deploy key");
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        assert_eq!(app.results()[0].source, "ops.md");
        assert!(app.results()[0].bm25 > 0.0);
        app.handle(&Event::Key(Key::Tab), &cx);
        app.handle(&Event::Key(Key::Ctrl('g')), &cx); // the classic single search and answer
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        assert_eq!(app.answer(), Some("It is in the vault [1]."));
        let mut g = Gfx::new(900, 600);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 80.0, 900.0, 520.0), 1.0, &cx, 1);
    }

    #[test]
    fn users_only_see_their_own_and_shared_namespaces() {
        let (mut cx, d) = cx_with_backend("rag2", iyakki_shared::chat::Unconfigured);
        let docs = d.join("docs");
        for (dir, file, text) in [("alice", "a.txt", "alice private plan zebra"), ("bob", "b.txt", "bob private plan zebra"), ("shared", "s.txt", "shared handbook zebra")] {
            std::fs::create_dir_all(docs.join(dir)).unwrap();
            std::fs::write(docs.join(dir).join(file), text).unwrap();
        }
        cx.cfg.rag_docs = docs.display().to_string();
        cx.services = services_in(&d, Arc::new(Offline));
        cx.session.user = "alice".into();
        // bob has already been indexed by an earlier session of his own
        cx.kb().add_document("user/bob", "b.txt", "b.txt", "bob private plan zebra", BTreeMap::new()).unwrap();
        let mut app = RagApp::new(&cx);
        settle(&mut app, &cx);
        typed(&mut app, &cx, "zebra");
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        let mut sources: Vec<&str> = app.results().iter().map(|h| h.source.as_str()).collect();
        sources.sort();
        assert_eq!(sources, vec!["a.txt", "s.txt"], "bob's document must not be visible even though it is indexed");
    }

    #[test]
    fn external_backends_contribute_and_failures_warn() {
        struct Net;
        impl Upstream for Net {
            fn request(&self, _m: &str, url: &str, _h: &[(String, String)], _b: &[u8]) -> Result<UpstreamResponse, String> {
                if url == "http://rag.corp/search" {
                    Ok(UpstreamResponse { status: 200, body: br#"{"results":[{"text":"Corporate wiki: zebra policy is documented here.","source":"wiki"}]}"#.to_vec() })
                } else {
                    Err("connection refused".into())
                }
            }
        }
        let (mut cx, d) = cx_with_backend("rag3", iyakki_shared::chat::Unconfigured);
        cx.services = services_in(&d, Arc::new(Net));
        {
            let mut st = cx.settings().lock().unwrap();
            st.upsert_external(ExtEntry { enabled: true, backend: ExternalRag::Http { id: "corp".into(), url: "http://rag.corp/search".into(), key_ref: None } }).unwrap();
            st.upsert_external(ExtEntry { enabled: true, backend: ExternalRag::Http { id: "down".into(), url: "http://down.corp/search".into(), key_ref: None } }).unwrap();
        }
        cx.cfg.rag_docs = d.join("docs").display().to_string();
        let mut app = RagApp::new(&cx);
        settle(&mut app, &cx);
        typed(&mut app, &cx, "zebra policy");
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        assert!(app.results().iter().any(|h| h.ns == "ext:corp" && h.source == "wiki"));
        assert!(app.warnings().iter().any(|w| w.starts_with("down:")), "{:?}", app.warnings());
        let mut g = Gfx::new(900, 600);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 80.0, 900.0, 520.0), 1.0, &cx, 1);
    }

    #[test]
    fn a_failed_model_call_in_ask_mode_is_shown_in_plain_words() {
        struct Refused;
        impl iyakki_shared::chat::ChatBackend for Refused {
            fn complete(&self, _m: &[iyakki_shared::chat::Message]) -> Result<String, String> {
                Err("HTTP 401: {\"detail\":\"bad key 123e4567-e89b-12d3-a456-426614174000\"}".into())
            }
        }
        let (mut cx, d) = cx_with_backend("rag5", Refused);
        let docs = d.join("docs");
        std::fs::create_dir_all(docs.join("shared")).unwrap();
        std::fs::write(docs.join("shared/a.txt"), "The zebra policy is documented here.").unwrap();
        cx.cfg.rag_docs = docs.display().to_string();
        cx.services = services_in(&d, Arc::new(Offline));
        let mut app = RagApp::new(&cx);
        settle(&mut app, &cx);
        app.handle(&Event::Key(Key::Tab), &cx);
        app.handle(&Event::Key(Key::Ctrl('g')), &cx); // the classic single search and answer
        typed(&mut app, &cx, "zebra policy");
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        let e = app.error.clone().unwrap_or_default();
        assert!(e.contains("Settings -> Models") && !e.contains("123e4567"), "{e}");
    }

    /// A model that plans, grades, rewrites and answers by what is asked of it.
    struct Researcher;
    impl ChatBackend for Researcher {
        fn complete(&self, h: &[Message]) -> Result<String, String> {
            let sys = &h[0].content;
            Ok(if sys.contains("plan document retrieval") {
                "[\"where is the deploy key stored\"]".into()
            } else if sys.contains("grade search results") {
                "{\"1\":\"relevant\",\"2\":\"partly\",\"3\":\"partly\"}".into()
            } else if sys.contains("rewrite a search query") {
                "deploy key vault".into()
            } else {
                "The deploy key is in the vault under prod [1].".into()
            })
        }
    }

    fn agentic_app(tag: &str) -> (RagApp, AppCx) {
        let (mut cx, d) = cx_with_backend(tag, Researcher);
        let docs = d.join("docs");
        std::fs::create_dir_all(docs.join("shared")).unwrap();
        std::fs::write(docs.join("shared/ops.md"), "The deploy key is stored in the vault under prod.").unwrap();
        std::fs::write(docs.join("shared/other.md"), "Lunch is served at noon in the canteen.").unwrap();
        cx.cfg.rag_docs = docs.display().to_string();
        cx.services = services_in(&d, Arc::new(Offline));
        let mut app = RagApp::new(&cx);
        settle(&mut app, &cx);
        (app, cx)
    }

    #[test]
    fn the_agentic_switch_runs_the_research_loop_and_the_evidence_page_shows_the_ledger() {
        let (mut app, cx) = agentic_app("ragag1");
        assert!(app.is_agentic(), "agentic is on by default in ask mode");
        app.handle(&Event::Key(Key::Tab), &cx);
        typed(&mut app, &cx, "where is the deploy key stored");
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        assert_eq!(app.answer(), Some("The deploy key is in the vault under prod [1]."));
        let l = app.ledger().expect("a ledger");
        assert_eq!(l.stop, iyakki_agent::agenticrag::Stop::Sufficient.text());
        assert_eq!(l.citations, 1);
        assert!(l.searches >= 1 && l.searches <= 12);
        assert_eq!(app.hit_tags.len(), app.results().len());
        assert!(app.hit_tags[0].starts_with("local · shared:"), "{:?}", app.hit_tags);
        let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap();
        assert!(log.contains("rag.research") && !log.contains("deploy key"), "the words are not logged: {log}");
        // the run is kept for the evidence page, also for the chat to find
        assert_eq!(iyakki_agent::ragtools::LedgerStore::open(cx.data_dir(), "tester").runs().len(), 1);
        // the evidence page: Ctrl+E opens, Esc closes
        app.handle(&Event::Key(Key::Ctrl('e')), &cx);
        assert!(app.ledger_page_open());
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(9, 11, 20));
        app.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 0);
        for label in ["Back", "Older", "Newer", "Up", "Down"] {
            assert!(g.hits.iter().any(|h| h.label == label), "{label}");
        }
        app.handle(&Event::Key(Key::Esc), &cx);
        assert!(!app.ledger_page_open());
        // switching it off gives the classic single search and answer
        app.handle(&Event::Key(Key::Ctrl('g')), &cx);
        assert!(!app.is_agentic());
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        assert!(app.ledger().is_none() && app.hit_tags.is_empty());
    }

    #[test]
    fn with_no_model_the_agentic_run_says_it_was_a_plain_search() {
        let (mut cx, d) = cx_with_backend("ragag2", iyakki_shared::chat::Unconfigured);
        let docs = d.join("docs");
        std::fs::create_dir_all(docs.join("shared")).unwrap();
        std::fs::write(docs.join("shared/ops.md"), "The deploy key is stored in the vault under prod.").unwrap();
        cx.cfg.rag_docs = docs.display().to_string();
        cx.services = services_in(&d, Arc::new(Offline));
        let mut app = RagApp::new(&cx);
        settle(&mut app, &cx);
        app.handle(&Event::Key(Key::Tab), &cx);
        typed(&mut app, &cx, "deploy key vault");
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        let l = app.ledger().unwrap();
        assert!(!l.agentic);
        assert!(l.summary.contains("no model") && app.answer().unwrap().contains("No model is set up"));
        assert!(!app.results().is_empty());
    }

    fn intersects(a: R, b: R) -> bool {
        a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
    }

    #[test]
    fn the_agentic_switch_and_the_evidence_page_fit_and_work_at_the_four_sizes() {
        let (mut app, cx) = agentic_app("ragag3");
        app.handle(&Event::Key(Key::Tab), &cx);
        typed(&mut app, &cx, "where is the deploy key stored");
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        for (w, h, s) in [(1280u32, 720u32, 1.0f32), (1920, 1080, 1.5), (1024, 600, 0.8), (800, 480, 0.62)] {
            let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
            let bounds = R::new(0.0, 0.0, w as f32, h as f32);
            let inside = |r: R| r.x >= -0.5 && r.y >= area.y - 0.5 && r.x + r.w <= bounds.w + 0.5 && r.y + r.h <= bounds.h + 0.5;
            // the result page
            let mut g = Gfx::new(w, h);
            g.clear(rgb(9, 11, 20));
            app.render(&mut g, area, s, &cx, 0);
            let top: Vec<_> = g.hits.iter().filter(|x| matches!(x.label.as_str(), "Agentic: on" | "Evidence" | "Documents" | "Ask" | "⇄ Search")).cloned().collect();
            assert_eq!(top.len(), 5, "{w}x{h}: {:?}", g.hits.iter().map(|x| x.label.clone()).collect::<Vec<_>>());
            for (i, a) in top.iter().enumerate() {
                assert!(inside(a.rect) && a.rect.w > 20.0 * s && a.rect.h > 14.0 * s, "{w}x{h}: {} {:?}", a.label, a.rect);
                for b in &top[i + 1..] {
                    assert!(!intersects(a.rect, b.rect), "{w}x{h}: {} overlaps {}: {:?} {:?}", a.label, b.label, a.rect, b.rect);
                }
            }
            // every result row stays on the screen
            for hit in g.hits.iter().filter(|x| x.label == "result") {
                assert!(inside(hit.rect), "{w}x{h}: result {:?}", hit.rect);
            }
            // mouse and touch deliver the button's event; the keyboard has a key for the same
            let sw = top.iter().find(|x| x.label == "Agentic: on").unwrap().ev.clone();
            app.handle(&sw, &cx);
            assert!(!app.is_agentic(), "{w}x{h}: the button switches it off");
            app.handle(&Event::Key(Key::Ctrl('g')), &cx);
            assert!(app.is_agentic());
            // the evidence page
            let ev = g.hits.iter().find(|x| x.label == "Evidence").unwrap().ev.clone();
            app.handle(&ev, &cx);
            assert!(app.ledger_page_open(), "{w}x{h}");
            let mut g = Gfx::new(w, h);
            g.clear(rgb(9, 11, 20));
            app.render(&mut g, area, s, &cx, 0);
            let btns: Vec<_> = g.hits.iter().filter(|x| matches!(x.label.as_str(), "Back" | "Down" | "Up" | "Newer" | "Older")).cloned().collect();
            assert_eq!(btns.len(), 5, "{w}x{h}");
            for (i, a) in btns.iter().enumerate() {
                assert!(inside(a.rect), "{w}x{h}: {} {:?}", a.label, a.rect);
                for b in &btns[i + 1..] {
                    assert!(!intersects(a.rect, b.rect), "{w}x{h}: {} overlaps {}", a.label, b.label);
                }
            }
            // scrolling by button, key and wheel never goes past the ends
            let down = btns.iter().find(|x| x.label == "Down").unwrap().ev.clone();
            for _ in 0..200 {
                app.handle(&down, &cx);
            }
            app.handle(&Event::Scroll(-50), &cx);
            app.render(&mut g, area, s, &cx, 0);
            assert!(app.lscroll <= app.lmax);
            app.handle(&Event::Scroll(500), &cx);
            assert_eq!(app.lscroll, 0);
            let back = btns.iter().find(|x| x.label == "Back").unwrap().ev.clone();
            app.handle(&back, &cx);
            assert!(!app.ledger_page_open(), "{w}x{h}");
        }
    }

    #[test]
    fn nothing_found_is_reported() {
        let (cx, _d) = cx_with_backend("rag4", iyakki_shared::chat::Unconfigured);
        let mut app = RagApp::new(&cx);
        settle(&mut app, &cx);
        typed(&mut app, &cx, "anything at all");
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        assert!(app.results().is_empty() && app.error.is_some());
        let _ = temp_dir("rag4-unused");
    }
}
