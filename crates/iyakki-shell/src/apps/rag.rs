//! RAG: search your documents (and any external RAG backends Settings enables), or ask the model a
//! question grounded in what was found. Each user searches their own namespace plus the shared one.
//! Searching and indexing run on worker threads, because embedding may call a remote model.

use std::sync::mpsc::{channel, Receiver};

use iyakki_agent::agenticrag::{research, Budget, Ledger, Mode, Request};
use iyakki_agent::kb::Hit;
use iyakki_agent::ragtools::{LedgerStore, RagAccess};
use iyakki_agent::rag::{build_prompt, Passage};

use super::widgets::render_confirm;
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(61, 214, 163);

type SearchOut = Result<(Option<String>, Vec<Hit>, Vec<String>), String>;

/// One indexed document in the Documents list.
#[derive(Clone, Debug)]
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

pub struct RagApp {
    /// The Documents list is open (instead of the search page).
    docs_open: bool,
    docs: Vec<DocRow>,
    dsel: usize,
    confirm: Option<DocRow>,
    status: Option<(String, bool)>,
    query: TextField,
    ask_mode: bool,
    hits: Vec<Hit>,
    warnings: Vec<String>,
    answer: Option<String>,
    /// Selecting and copying the model's answer.
    pane: crate::textsel::Pane,
    error: Option<String>,
    pending: Option<Receiver<SearchOut>>,
    indexing: Option<Receiver<String>>,
    info: String,
    /// Ask mode runs the bounded research loop (plan, search, grade, rewrite, cite) instead of one search.
    agentic: bool,
    researching: Option<Receiver<Ledger>>,
    /// The run on screen, and the evidence page that shows what it did.
    ledger: Option<Ledger>,
    ledger_open: bool,
    /// Earlier runs of this account (also the ones the chat made), newest first, and which one the page shows (0 = the run on screen).
    runs: Vec<Ledger>,
    lrun: usize,
    lscroll: usize,
    /// The most the evidence page can scroll (set when it is drawn).
    lmax: usize,
    /// What each result row says about where it came from (agentic runs); empty for a plain search.
    hit_tags: Vec<String>,
}

impl RagApp {
    /// Namespaces this user searches.
    pub fn namespaces(cx: &AppCx) -> Vec<String> {
        vec![format!("user/{}", cx.session.user), "shared".to_string()]
    }

    pub fn new(cx: &AppCx) -> RagApp {
        let mut app = RagApp { docs_open: false, docs: vec![], dsel: 0, confirm: None, status: None, query: TextField::new("Search your documents…", false), ask_mode: false, hits: vec![], warnings: vec![], answer: None, pane: crate::textsel::Pane::new(61_300), error: None, pending: None, indexing: None, info: String::new(), agentic: true, researching: None, ledger: None, ledger_open: false, runs: vec![], lrun: 0, lscroll: 0, lmax: 0, hit_tags: vec![] };
        // index the documents folders in the background (idempotent: unchanged files are skipped)
        let (tx, rx) = channel();
        let svc = cx.services.clone();
        let base = std::path::PathBuf::from(&cx.cfg.rag_docs);
        let user = cx.session.user.clone();
        std::thread::spawn(move || {
            let mut kb = svc.kb.lock().unwrap();
            let (a1, _) = kb.ingest_dir(&format!("user/{user}"), &base.join(&user));
            let (a2, _) = kb.ingest_dir("shared", &base.join("shared"));
            let docs: usize = ["shared".to_string(), format!("user/{user}")].iter().filter_map(|n| kb.stats(n)).map(|s| s.docs).sum();
            let _ = tx.send(format!("{docs} documents indexed{}", if a1 + a2 > 0 { format!(" ({} new)", a1 + a2) } else { String::new() }));
        });
        app.indexing = Some(rx);
        app
    }

    pub fn docs(&self) -> &[DocRow] {
        &self.docs
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    /// The documents of this account's own namespace and the shared one.
    fn load_docs(&mut self, cx: &AppCx) {
        let kb = cx.services.kb.lock().unwrap();
        let mut v = Vec::new();
        for ns in Self::namespaces(cx).into_iter().rev() {
            for d in kb.docs(&ns) {
                v.push(DocRow { ns: ns.clone(), id: d.id, title: d.title, source: d.source, chunks: d.chunks });
            }
        }
        v.sort_by(|a, b| (a.ns != "shared", &a.title).cmp(&(b.ns != "shared", &b.title)));
        self.docs = v;
        self.dsel = self.dsel.min(self.docs.len().saturating_sub(1));
    }

    /// Whether this account may delete the document: its own, or a shared one when it manages the system.
    fn can_delete(cx: &AppCx, d: &DocRow) -> bool {
        d.ns == format!("user/{}", cx.session.user) || (d.ns == "shared" && cx.session.policy.manage_settings)
    }

    /// Removes the document from the index and, when it came from a file in the documents folder, deletes the file so it is not
    /// indexed again.
    fn delete_doc(&mut self, cx: &AppCx, d: &DocRow) {
        if !Self::can_delete(cx, d) {
            self.status = Some(("You may not delete that document.".into(), false));
            return;
        }
        if let Err(e) = cx.services.kb.lock().unwrap().remove_document(&d.ns, &d.id) {
            self.status = Some((format!("Could not remove it: {e}"), false));
            return;
        }
        let base = std::path::PathBuf::from(&cx.cfg.rag_docs).join(if d.ns == "shared" { "shared".to_string() } else { cx.session.user.clone() });
        let file = base.join(&d.source);
        // only a plain file inside the documents folder is touched
        if let (Ok(b), Ok(f)) = (std::fs::canonicalize(&base), std::fs::canonicalize(&file)) {
            if f.starts_with(&b) && f.is_file() {
                let _ = std::fs::remove_file(&f);
            }
        }
        cx.audit.record(&cx.session.user, "rag.delete", &d.source);
        self.status = Some((format!("Deleted {}.", d.title), true));
        self.load_docs(cx);
    }

    pub fn results(&self) -> &[Hit] {
        &self.hits
    }
    pub fn answer(&self) -> Option<&str> {
        self.answer.as_deref()
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub fn is_waiting(&self) -> bool {
        self.pending.is_some() || self.indexing.is_some() || self.researching.is_some()
    }
    pub fn is_agentic(&self) -> bool {
        self.agentic
    }
    pub fn ledger(&self) -> Option<&Ledger> {
        self.ledger.as_ref()
    }
    pub fn ledger_page_open(&self) -> bool {
        self.ledger_open
    }

    /// The runs the evidence page can show: the one on screen first, then the kept ones (chat runs too) without repeating it.
    fn load_runs(&mut self, cx: &AppCx) {
        let kept = LedgerStore::open(cx.services.dir(), &cx.session.user).runs();
        self.runs = self.ledger.iter().cloned().chain(kept.into_iter().filter(|k| self.ledger.as_ref().is_none_or(|l| l.run_id != k.run_id))).collect();
        self.lrun = self.lrun.min(self.runs.len().saturating_sub(1));
    }

    fn open_ledger(&mut self, cx: &AppCx) {
        self.load_runs(cx);
        if !self.runs.is_empty() {
            self.ledger_open = true;
            self.lrun = 0;
            self.lscroll = 0;
        } else {
            self.error = Some("No research has been run yet. Ask a question with Agentic on.".into());
        }
    }

    fn start_research(&mut self, cx: &AppCx, q: String) {
        let (tx, rx) = channel();
        let (svc, backend, user) = (cx.services.clone(), cx.chat.clone(), cx.session.user.clone());
        std::thread::spawn(move || {
            let access = RagAccess::for_services(&svc, &user);
            let k = svc.settings.lock().unwrap().get().rag.top_k;
            let l = research(&access, Some(backend.as_ref()), &Request { question: &q, collections: &[], budget: Budget { k, ..Budget::default() } }, &std::sync::atomic::AtomicBool::new(false));
            LedgerStore::open(svc.dir(), &user).push(&l);
            let _ = tx.send(l);
        });
        self.researching = Some(rx);
    }

    /// The result rows of a finished run: its kept passages, each with where it came from.
    fn show_ledger(&mut self, l: Ledger) {
        self.hits = l
            .passages
            .iter()
            .map(|p| Hit { ns: if let Some(e) = p.backend.strip_prefix("external:") { format!("ext:{e}") } else { p.collection.clone() }, doc_id: p.id.clone(), title: p.title.clone(), source: p.source.clone(), chunk_idx: 0, text: p.text.clone(), score: p.score, bm25: 0.0, vector: 0.0 })
            .collect();
        self.hit_tags = l.passages.iter().map(|p| format!("{} · {}", p.backend, p.id)).collect();
        self.warnings = l.warnings.clone();
        self.answer = Some(l.answer.clone());
        self.error = None;
        self.ledger = Some(l);
    }

    fn submit(&mut self, cx: &AppCx) {
        let q = self.query.value.trim().to_string();
        if q.is_empty() || self.pending.is_some() || self.researching.is_some() {
            return;
        }
        self.error = None;
        self.answer = None;
        self.hit_tags.clear();
        if self.ask_mode && self.agentic {
            cx.audit.record(&cx.session.user, "rag.research", &format!("{} chars", q.len()));
            self.hits.clear();
            self.warnings.clear();
            self.ledger = None;
            self.start_research(cx, q);
            return;
        }
        self.ledger = None;
        cx.audit.record(&cx.session.user, if self.ask_mode { "rag.ask" } else { "rag.search" }, &format!("{} chars", q.len()));
        let (tx, rx) = channel();
        let (svc, backend, ask) = (cx.services.clone(), cx.chat.clone(), self.ask_mode);
        let nss = Self::namespaces(cx);
        std::thread::spawn(move || {
            let k = svc.settings.lock().unwrap().get().rag.top_k;
            let refs: Vec<&str> = nss.iter().map(String::as_str).collect();
            let (hits, warnings) = svc.search(&refs, &q, k);
            if hits.is_empty() {
                let _ = tx.send(Err(if warnings.is_empty() { "No matching passages.".into() } else { format!("No results. {}", warnings.join("; ")) }));
                return;
            }
            let answer = if ask {
                let ps: Vec<Passage> = hits.iter().take(k.min(6)).map(Passage::from).collect();
                match backend.complete(&build_prompt(&q, &ps)) {
                    Ok(a) => Some(a),
                    Err(e) => {
                        let _ = tx.send(Err(iyakki_agent::providererr::plain_error(&backend.provider_name(), &e)));
                        return;
                    }
                }
            } else {
                None
            };
            let _ = tx.send(Ok((answer, hits, warnings)));
        });
        self.pending = Some(rx);
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.pending {
            if let Ok(r) = rx.try_recv() {
                self.pending = None;
                match r {
                    Ok((a, hits, w)) => {
                        self.answer = a;
                        self.hits = hits;
                        self.warnings = w;
                    }
                    Err(e) => {
                        self.hits.clear();
                        self.error = Some(e);
                    }
                }
            }
        }
        if let Some(rx) = &self.researching {
            if let Ok(l) = rx.try_recv() {
                self.researching = None;
                self.show_ledger(l);
            }
        }
        if let Some(rx) = &self.indexing {
            if let Ok(m) = rx.try_recv() {
                self.info = m;
                self.indexing = None;
            }
        }
    }
}

impl RagApp {
    /// The evidence page: what a research run did, line by line. The budget used and the stop reason stay at the top; the rest scrolls
    /// (arrow keys, the wheel, a finger drag, or the Up and Down buttons).
    fn draw_ledger(&mut self, g: &mut Gfx, area: R, s: f32) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 8.0 * s;
        let Some(l) = self.runs.get(self.lrun).cloned() else {
            self.ledger_open = false;
            return;
        };
        let title = format!("Evidence ledger · run {} of {}", self.lrun + 1, self.runs.len());
        let bsize = 13.0 * s;
        let bw_of = |g: &mut Gfx, label: &str| g.text_width(bsize, label, Face::Bold) + bsize * 1.7;
        // buttons on the right, from the edge inwards
        let mut bx = x + w;
        for (label, ev) in [("Back", Event::Tab(63)), ("Down", Event::Tab(67)), ("Up", Event::Tab(66)), ("Newer", Event::Tab(65)), ("Older", Event::Tab(64))] {
            let bw = bw_of(g, label);
            bx -= bw;
            g.button(bx, y, bsize, label, MUTED, ev);
            bx -= 6.0 * s;
        }
        let t = g.fit(20.0 * s, &title, Face::Bold, (bx - x).max(60.0 * s));
        g.text(x, y + 2.0 * s, 20.0 * s, TEXT, &t, Face::Bold);
        y += 40.0 * s;
        for line in g.wrap(14.0 * s, &l.summary_line(), Face::Bold, w) {
            g.text(x, y, 14.0 * s, if l.mode == Mode::Agentic { ACCENT } else { rgb(255, 196, 94) }, &line, Face::Bold);
            y += 20.0 * s;
        }
        y += 6.0 * s;
        // every line of the ledger, wrapped; the question and the summary are shown above, so they are skipped here
        let size = 14.0 * s;
        let lh = 19.0 * s;
        let mut rows: Vec<(String, bool)> = vec![];
        for (i, line) in l.lines().into_iter().enumerate() {
            if i == 0 {
                continue;
            }
            let head = !line.starts_with(' ') && !line.starts_with('[') && !line.starts_with("note");
            for r in g.wrap(size, &line, Face::Sans, w - 8.0 * s) {
                rows.push((r, head));
            }
        }
        let fit = (((area.y + area.h - y - 30.0 * s) / lh).floor() as usize).max(1);
        self.lmax = rows.len().saturating_sub(fit);
        self.lscroll = self.lscroll.min(self.lmax);
        for (r, head) in rows.iter().skip(self.lscroll).take(fit) {
            g.text(x, y, size, if *head { TEXT } else { MUTED }, r, Face::Sans);
            y += lh;
        }
        let hint = if self.lmax > 0 { format!("lines {}-{} of {} · Up/Down scroll · Left/Right other runs · Esc back", self.lscroll + 1, (self.lscroll + fit).min(rows.len()), rows.len()) } else { "Left/Right other runs · Esc back".to_string() };
        let hint = g.fit(12.0 * s, &hint, Face::Sans, w);
        g.text(x, area.y + area.h - 22.0 * s, 12.0 * s, MUTED, &hint, Face::Sans);
    }

    fn draw_docs(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 8.0 * s;
        g.text(x, y + 2.0 * s, 20.0 * s, TEXT, &format!("Your documents ({})", self.docs.len()), Face::Bold);
        g.button(x + w - 130.0 * s, y, 13.0 * s, "Back to search", MUTED, Event::Tab(60));
        y += 36.0 * s;
        g.text(x, y, 13.0 * s, MUTED, "These are what the assistant can search. Delete the ones you do not need; samples are marked.", Face::Sans);
        y += 28.0 * s;
        if let Some((m, ok)) = &self.status {
            crate::fx::status_line(g, x, y, w, s, m, *ok);
            y += 24.0 * s;
        }
        if self.docs.is_empty() {
            crate::fx::empty_state(g, R::new(x, y, w, 220.0 * s), s, Icon::Rag, "No documents yet", "Put .txt or .md files in the documents folder and open RAG again.", ACCENT);
        }
        let rh = 56.0 * s;
        let fit = (((area.y + area.h - y - 64.0 * s) / (rh + 6.0 * s)).floor() as usize).max(1);
        let start = self.dsel.saturating_sub(fit - 1);
        for (i, d) in self.docs.iter().enumerate().skip(start).take(fit) {
            let r = R::new(x, y, w, rh);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, i == self.dsel, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &d.title);
            g.rect(r.x, r.y + 10.0 * s, 4.0 * s, r.h - 20.0 * s, if d.ns == "shared" { rgb(120, 170, 255) } else { ACCENT });
            let title = g.fit(16.0 * s, &d.title, Face::Bold, w - 260.0 * s);
            g.text(r.x + 18.0 * s, r.y + 8.0 * s, 16.0 * s, TEXT, &title, Face::Bold);
            let tag = format!("{}{} · {} passage{}", if d.is_sample() { "SAMPLE · " } else { "" }, if d.ns == "shared" { "shared" } else { "mine" }, d.chunks, if d.chunks == 1 { "" } else { "s" });
            g.text(r.x + 18.0 * s, r.y + 32.0 * s, 12.0 * s, if d.is_sample() { rgb(255, 196, 94) } else { MUTED }, &tag, Face::Sans);
            if i == self.dsel && Self::can_delete(cx, d) {
                g.button(r.x + r.w - 124.0 * s, r.y + 11.0 * s, 13.0 * s, "Delete (d)", ERR, Event::Tab(61));
            }
            y += rh + 6.0 * s;
        }
        if let Some(d) = &self.confirm {
            render_confirm(g, area, s, &format!("Delete “{}” from your documents?", d.title), "Delete");
        }
    }
}

impl App for RagApp {
    fn title(&self) -> &'static str {
        "RAG"
    }
    fn icon(&self) -> Icon {
        Icon::Rag
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "Enter run · Tab search/ask · Ctrl+O your documents (delete the ones you do not need) · click a result to copy · Esc back".into()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        // the Documents list
        if let Some(d) = self.confirm.clone() {
            if matches!(ev, Event::Key(Key::Char('y')) | Event::Key(Key::Enter)) {
                self.confirm = None;
                self.delete_doc(cx, &d);
            } else if matches!(ev, Event::Key(_)) {
                self.confirm = None;
            }
            return Flow::Stay;
        }
        match ev {
            Event::Tab(60) | Event::Key(Key::Ctrl('o')) => {
                self.docs_open = !self.docs_open;
                self.status = None;
                if self.docs_open {
                    self.load_docs(cx);
                }
                return Flow::Stay;
            }
            Event::Tab(61) if self.docs_open => {
                if let Some(d) = self.docs.get(self.dsel).cloned() {
                    self.confirm = Some(d);
                }
                return Flow::Stay;
            }
            _ => {}
        }
        // the evidence page
        if self.ledger_open {
            let last = self.lmax;
            match ev {
                Event::Tab(63) | Event::Key(Key::Esc) | Event::Key(Key::Ctrl('e')) => self.ledger_open = false,
                Event::Tab(64) | Event::Key(Key::Right) => {
                    if self.lrun + 1 < self.runs.len() {
                        self.lrun += 1;
                        self.lscroll = 0;
                    }
                }
                Event::Tab(65) | Event::Key(Key::Left) => {
                    self.lrun = self.lrun.saturating_sub(1);
                    self.lscroll = 0;
                }
                Event::Tab(66) | Event::Key(Key::Up) => self.lscroll = self.lscroll.saturating_sub(1),
                Event::Tab(67) | Event::Key(Key::Down) => self.lscroll = (self.lscroll + 1).min(last),
                Event::Key(Key::PageUp) => self.lscroll = self.lscroll.saturating_sub(8),
                Event::Key(Key::PageDown) => self.lscroll = (self.lscroll + 8).min(last),
                Event::Scroll(n) => self.lscroll = if *n > 0 { self.lscroll.saturating_sub(*n as usize * 3) } else { (self.lscroll + n.unsigned_abs() as usize * 3).min(last) },
                _ => {}
            }
            return Flow::Stay;
        }
        if matches!(ev, Event::Tab(62) | Event::Key(Key::Ctrl('g'))) && !self.docs_open {
            self.agentic = !self.agentic;
            if self.agentic {
                self.ask_mode = true;
            }
            return Flow::Stay;
        }
        if matches!(ev, Event::Tab(63) | Event::Key(Key::Ctrl('e'))) && !self.docs_open {
            self.open_ledger(cx);
            return Flow::Stay;
        }
        if !self.docs_open {
            if self.pane.handle(ev) {
                return Flow::Stay;
            }
            if matches!(ev, Event::Key(Key::Ctrl('a'))) && self.query.value.is_empty() {
                self.pane.select_all();
                return Flow::Stay;
            }
        }
        if self.docs_open && !matches!(ev, Event::Tick) {
            match ev {
                Event::Key(Key::Esc) => self.docs_open = false,
                Event::Key(Key::Up) => self.dsel = self.dsel.saturating_sub(1),
                Event::Key(Key::Down) => self.dsel = (self.dsel + 1).min(self.docs.len().saturating_sub(1)),
                Event::Row(i) => self.dsel = (*i).min(self.docs.len().saturating_sub(1)),
                Event::Key(Key::Char('d')) | Event::Key(Key::Delete) => {
                    if let Some(d) = self.docs.get(self.dsel).cloned() {
                        if Self::can_delete(cx, &d) {
                            self.confirm = Some(d);
                        } else {
                            self.status = Some(("Only an administrator can delete shared documents.".into(), false));
                        }
                    }
                }
                _ => {}
            }
            return Flow::Stay;
        }
        match ev {
            Event::Tick => self.poll(),
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Tab) => self.ask_mode = !self.ask_mode,
            Event::Key(Key::Enter) => self.submit(cx),
            Event::Paste(t) => {
                self.query.paste(t);
            }
            Event::FocusAt(_, x) => self.query.click(*x),
            // click on a result: copy its text
            Event::Row(i) => {
                if let Some(h) = self.hits.get(*i) {
                    crate::ui::clipboard::copy(&h.text);
                }
            }
            Event::Key(k) => {
                self.query.handle(k);
            }
            _ => {}
        }
        Flow::Stay
    }

    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        if self.docs_open {
            Vec::new()
        } else {
            self.pane.context_items(x, y)
        }
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.pane.begin(R::new(0.0, 0.0, 0.0, 0.0));
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 8.0 * s;
        if self.docs_open {
            self.draw_docs(g, area, s, cx);
            return;
        }
        if self.ledger_open {
            self.draw_ledger(g, area, s);
            return;
        }
        let mode = if self.ask_mode { if self.agentic { "Ask the model (agentic)" } else { "Ask the model" } } else { "Search" };
        let ext = cx.services.settings.lock().unwrap().get().external_rag.iter().filter(|e| e.enabled).count();
        let info = if self.indexing.is_some() { "indexing documents…".to_string() } else { self.info.clone() };
        let info = format!("{info}{}   mode: {mode}", if ext > 0 { format!("  ·  {ext} external backend(s)") } else { String::new() });
        // the buttons of the top row sit at the right; the status text takes what is left
        let docs_x = x + w - 120.0 * s;
        let bsize = 13.0 * s;
        let bw_of = |g: &mut Gfx, label: &str| g.text_width(bsize, label, Face::Bold) + bsize * 1.7;
        let mut left = docs_x - 8.0 * s;
        let a_label = if self.agentic { "Agentic: on" } else { "Agentic: off" };
        let aw = bw_of(g, a_label);
        left -= aw;
        g.button(left, y, bsize, a_label, if self.agentic { ACCENT } else { MUTED }, Event::Tab(62));
        let ew = bw_of(g, "Evidence");
        left -= ew + 8.0 * s;
        g.button(left, y, bsize, "Evidence", MUTED, Event::Tab(63));
        let shown = g.fit(15.0 * s, &info, Face::Sans, (left - 8.0 * s - x).max(40.0 * s));
        g.text(x, y, 15.0 * s, MUTED, &shown, Face::Sans);
        y += 28.0 * s;
        let bw = 120.0 * s;
        self.query.render(g, R::new(x, y, w - bw - 12.0 * s - 116.0 * s, 46.0 * s), true, s, blink(tick), Face::Sans, 0);
        g.button(docs_x, y - 28.0 * s, 13.0 * s, "Documents", MUTED, Event::Tab(60));
        let (bx, by) = (x + w - bw - 116.0 * s, y + 6.0 * s);
        g.button(bx, by, 15.0 * s, if self.ask_mode { "Ask" } else { "Search" }, ACCENT, Event::Key(Key::Enter));
        g.button(bx + 112.0 * s, by, 15.0 * s, if self.ask_mode { "⇄ Search" } else { "⇄ Ask" }, MUTED, Event::Key(Key::Tab));
        y += 62.0 * s;
        if self.pending.is_some() || self.researching.is_some() {
            let word = if self.researching.is_some() { "researching" } else if self.ask_mode { "thinking" } else { "searching" };
            crate::fx::chip(g, x, y - 2.0 * s, s, &format!("{word}{}", ".".repeat(1 + (tick / 10 % 3) as usize)), crate::fx::CYAN, false);
            y += 30.0 * s;
        }
        // the budget used and why the run stopped are always shown
        if let Some(l) = &self.ledger {
            let line = g.fit(13.0 * s, &l.summary_line(), Face::Sans, w);
            g.text(x, y, 13.0 * s, if l.mode == Mode::Agentic { ACCENT } else { rgb(255, 196, 94) }, &line, Face::Sans);
            y += 22.0 * s;
        }
        if let Some(e) = &self.error {
            for l in g.wrap(16.0 * s, e, Face::Sans, w) {
                g.text(x, y, 16.0 * s, ERR, &l, Face::Sans);
                y += 22.0 * s;
            }
            y += 8.0 * s;
        }
        for wmsg in self.warnings.iter().take(2) {
            let m = g.fit(13.0 * s, &format!("⚠ {wmsg}"), Face::Sans, w);
            g.text(x, y, 13.0 * s, rgb(255, 196, 94), &m, Face::Sans);
            y += 20.0 * s;
        }
        if let Some(a) = self.answer.clone() {
            let lines = g.wrap(17.0 * s, &a, Face::Sans, w - 32.0 * s);
            let lh = g.line_height(17.0 * s, Face::Sans);
            let h = (lines.len() as f32 * lh + 28.0 * s).min(area.y + area.h - y - 120.0 * s).max(60.0 * s);
            crate::fx::card(g, R::new(x, y, w, h), s, ACCENT, 0.4);
            self.pane.begin(R::new(x, y, w, h));
            for (i, l) in lines.iter().enumerate().take(((h - 20.0 * s) / lh) as usize) {
                g.text(x + 16.0 * s, y + 14.0 * s + i as f32 * lh, 17.0 * s, TEXT, l, Face::Sans);
                self.pane.line(g, x + 16.0 * s, y + 14.0 * s + i as f32 * lh, lh, 17.0 * s, Face::Sans, l, 0, 0);
            }
            self.pane.end(g, ACCENT);
            y += h + 14.0 * s;
        }
        for (i, hit) in self.hits.iter().enumerate() {
            if y > area.y + area.h - 70.0 * s {
                break;
            }
            let ch = 74.0 * s;
            crate::fx::row_at(g, x, y, w, ch, 12.0 * s, false, g.hovered(R::new(x, y, w, ch)), ACCENT);
            g.hit(R::new(x, y, w, ch), Event::Row(i), "result");
            g.rect(x, y + 10.0 * s, 4.0 * s, ch - 20.0 * s, if hit.ns.starts_with("ext:") { rgb(167, 116, 255) } else { ACCENT });
            let src = g.fit(14.0 * s, if hit.source.is_empty() { &hit.title } else { &hit.source }, Face::Bold, w * 0.5);
            let origin = if hit.ns.starts_with("ext:") { hit.ns.trim_start_matches("ext:").to_string() } else { "local".to_string() };
            g.text(x + 16.0 * s, y + 8.0 * s, 14.0 * s, ACCENT, &format!("[{}] {}", i + 1, src), Face::Bold);
            let tag = match self.hit_tags.get(i) {
                Some(t) => g.fit(12.0 * s, t, Face::Sans, w * 0.42),
                None => format!("{origin}  ·  keyword {:.1}  vector {:.2}", hit.bm25, hit.vector),
            };
            let tw = g.text_width(12.0 * s, &tag, Face::Sans);
            g.text(x + w - tw - 16.0 * s, y + 10.0 * s, 12.0 * s, if origin == "local" { OK } else { rgb(167, 116, 255) }, &tag, Face::Sans);
            let flat = hit.text.replace('\n', " ");
            let first = g.fit(15.0 * s, &flat, Face::Sans, w - 32.0 * s);
            g.text(x + 16.0 * s, y + 32.0 * s, 15.0 * s, TEXT, &first, Face::Sans);
            let rest: String = flat.chars().skip(first.chars().count().saturating_sub(1)).collect();
            if !rest.is_empty() && !first.ends_with('…') {
                let rest = g.fit(15.0 * s, &rest, Face::Sans, w - 32.0 * s);
                g.text(x + 16.0 * s, y + 52.0 * s, 15.0 * s, MUTED, &rest, Face::Sans);
            }
            y += ch + 10.0 * s;
        }
        if self.hits.is_empty() && self.error.is_none() && self.pending.is_none() && self.indexing.is_none() {
            let dirs = format!("{}/{}  and  {}/shared", cx.cfg.rag_docs, cx.session.user, cx.cfg.rag_docs);
            crate::fx::empty_state(g, R::new(x, y, w, (area.y + area.h - y).max(160.0 * s)), s, Icon::Rag, "Ask your documents", &format!("Type a question or keywords. Documents come from {dirs} (.txt/.md/…) and from external backends enabled in Settings."), ACCENT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_backend, services_in, temp_dir, Offline};
    use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
    use iyakki_agent::chat::{ChatBackend, Message};
    use iyakki_agent::rag_ext::ExternalRag;
    use iyakki_agent::settings::ExtEntry;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[test]
    fn the_documents_list_shows_samples_and_deletes_a_document_and_its_file_after_asking() {
        let (mut cx, d) = cx_with_backend("ragdocs", iyakki_agent::chat::Unconfigured);
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
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
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
        for _ in 0..400 {
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
        let (mut cx, d) = cx_with_backend("rag2", iyakki_agent::chat::Unconfigured);
        let docs = d.join("docs");
        for (dir, file, text) in [("alice", "a.txt", "alice private plan zebra"), ("bob", "b.txt", "bob private plan zebra"), ("shared", "s.txt", "shared handbook zebra")] {
            std::fs::create_dir_all(docs.join(dir)).unwrap();
            std::fs::write(docs.join(dir).join(file), text).unwrap();
        }
        cx.cfg.rag_docs = docs.display().to_string();
        cx.services = services_in(&d, Arc::new(Offline));
        cx.session.user = "alice".into();
        // bob has already been indexed by an earlier session of his own
        cx.services.kb.lock().unwrap().add_document("user/bob", "b.txt", "b.txt", "bob private plan zebra", BTreeMap::new()).unwrap();
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
        let (mut cx, d) = cx_with_backend("rag3", iyakki_agent::chat::Unconfigured);
        cx.services = services_in(&d, Arc::new(Net));
        {
            let mut st = cx.services.settings.lock().unwrap();
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
        impl iyakki_agent::chat::ChatBackend for Refused {
            fn complete(&self, _m: &[iyakki_agent::chat::Message]) -> Result<String, String> {
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
        assert_eq!(l.stop, iyakki_agent::agenticrag::Stop::Sufficient);
        assert_eq!(l.citations.len(), 1);
        assert!(l.searches >= 1 && l.searches <= 12);
        assert_eq!(app.hit_tags.len(), app.results().len());
        assert!(app.hit_tags[0].starts_with("local · shared:"), "{:?}", app.hit_tags);
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("rag.research") && !log.contains("deploy key"), "the words are not logged: {log}");
        // the run is kept for the evidence page, also for the chat to find
        assert_eq!(iyakki_agent::ragtools::LedgerStore::open(cx.services.dir(), "tester").runs().len(), 1);
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
        let (mut cx, d) = cx_with_backend("ragag2", iyakki_agent::chat::Unconfigured);
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
        assert_eq!(l.mode, iyakki_agent::agenticrag::Mode::Plain);
        assert!(l.summary_line().contains("no model") && app.answer().unwrap().contains("No model is set up"));
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
        let (cx, _d) = cx_with_backend("rag4", iyakki_agent::chat::Unconfigured);
        let mut app = RagApp::new(&cx);
        settle(&mut app, &cx);
        typed(&mut app, &cx, "anything at all");
        app.handle(&Event::Key(Key::Enter), &cx);
        settle(&mut app, &cx);
        assert!(app.results().is_empty() && app.error.is_some());
        let _ = temp_dir("rag4-unused");
    }
}
