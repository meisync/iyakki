//! The RAG tile: search your documents (and any external RAG backends Settings enables), or ask the model a question grounded in what was
//! found. Each person searches their own namespace plus the shared one. Searching and indexing run on worker threads started by the port,
//! because embedding may call a remote model. The tile names no `Services`, session or audit log: everything it needs comes through
//! [`RagPort`] (a fake in the tests, `apps/rag_port.rs` in the shell).

use std::sync::mpsc::{channel, Receiver};

use iyakki_tile_api::rag::{DocRow, HitRow, RunView, SearchOut};
pub use iyakki_tile_api::rag::RagPort;
use iyakki_uikit::blink::blink;
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::ui::{Event, Key, TextField, ERR, MUTED, OK, R, TEXT};
use iyakki_uikit::widgets::render_confirm;

pub const ACCENT: [u8; 4] = rgb(61, 214, 163);

/// What the tile tells the shell after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Stay,
    Back,
}

pub struct RagTile {
    /// The Documents list is open (instead of the search page).
    pub docs_open: bool,
    pub docs: Vec<DocRow>,
    pub dsel: usize,
    pub confirm: Option<DocRow>,
    pub status: Option<(String, bool)>,
    pub query: TextField,
    pub ask_mode: bool,
    pub hits: Vec<HitRow>,
    pub warnings: Vec<String>,
    pub answer: Option<String>,
    /// Selecting and copying the model's answer.
    pub pane: iyakki_uikit::textsel::Pane,
    pub error: Option<String>,
    pub pending: Option<Receiver<SearchOut>>,
    pub indexing: Option<Receiver<String>>,
    pub info: String,
    /// Ask mode runs the bounded research loop (plan, search, grade, rewrite, cite) instead of one search.
    pub agentic: bool,
    pub researching: Option<Receiver<RunView>>,
    /// The run on screen, and the evidence page that shows what it did.
    pub ledger: Option<RunView>,
    pub ledger_open: bool,
    /// Earlier runs of this account (also the ones the chat made), newest first, and which one the page shows (0 = the run on screen).
    pub runs: Vec<RunView>,
    pub lrun: usize,
    pub lscroll: usize,
    /// The most the evidence page can scroll (set when it is drawn).
    pub lmax: usize,
    /// What each result row says about where it came from (agentic runs); empty for a plain search.
    pub hit_tags: Vec<String>,
}

impl RagTile {
    pub fn new(port: &dyn RagPort) -> RagTile {
        let mut app = RagTile { docs_open: false, docs: vec![], dsel: 0, confirm: None, status: None, query: TextField::new("Search your documents…", false), ask_mode: false, hits: vec![], warnings: vec![], answer: None, pane: iyakki_uikit::textsel::Pane::new(61_300), error: None, pending: None, indexing: None, info: String::new(), agentic: true, researching: None, ledger: None, ledger_open: false, runs: vec![], lrun: 0, lscroll: 0, lmax: 0, hit_tags: vec![] };
        // index the documents folders in the background (idempotent: unchanged files are skipped)
        let (tx, rx) = channel();
        port.index(tx);
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
    pub fn load_docs(&mut self, port: &dyn RagPort) {
        let mut v = port.docs();
        v.sort_by(|a, b| (a.ns != "shared", &a.title).cmp(&(b.ns != "shared", &b.title)));
        self.docs = v;
        self.dsel = self.dsel.min(self.docs.len().saturating_sub(1));
    }

    /// Removes the document through the port (index, file and audit line) and reloads the list.
    pub fn delete_doc(&mut self, port: &dyn RagPort, d: &DocRow) {
        if !port.can_delete(d) {
            self.status = Some(("You may not delete that document.".into(), false));
            return;
        }
        if let Err(e) = port.delete_doc(d) {
            self.status = Some((format!("Could not remove it: {e}"), false));
            return;
        }
        self.status = Some((format!("Deleted {}.", d.title), true));
        self.load_docs(port);
    }

    pub fn results(&self) -> &[HitRow] {
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
    pub fn ledger(&self) -> Option<&RunView> {
        self.ledger.as_ref()
    }
    pub fn ledger_page_open(&self) -> bool {
        self.ledger_open
    }

    /// The runs the evidence page can show: the one on screen first, then the kept ones (chat runs too) without repeating it.
    pub fn load_runs(&mut self, port: &dyn RagPort) {
        let kept = port.runs();
        self.runs = self.ledger.iter().cloned().chain(kept.into_iter().filter(|k| self.ledger.as_ref().is_none_or(|l| l.run_id != k.run_id))).collect();
        self.lrun = self.lrun.min(self.runs.len().saturating_sub(1));
    }

    fn open_ledger(&mut self, port: &dyn RagPort) {
        self.load_runs(port);
        if !self.runs.is_empty() {
            self.ledger_open = true;
            self.lrun = 0;
            self.lscroll = 0;
        } else {
            self.error = Some("No research has been run yet. Ask a question with Agentic on.".into());
        }
    }

    /// The result rows of a finished run: its kept passages, each with where it came from.
    pub fn show_ledger(&mut self, l: RunView) {
        self.hits = l.hits();
        self.hit_tags = l.tags();
        self.warnings = l.warnings.clone();
        self.answer = Some(l.answer.clone());
        self.error = None;
        self.ledger = Some(l);
    }

    pub fn submit(&mut self, port: &dyn RagPort) {
        let q = self.query.value.trim().to_string();
        if q.is_empty() || self.pending.is_some() || self.researching.is_some() {
            return;
        }
        self.error = None;
        self.answer = None;
        self.hit_tags.clear();
        if self.ask_mode && self.agentic {
            self.hits.clear();
            self.warnings.clear();
            self.ledger = None;
            let (tx, rx) = channel();
            port.research(&q, tx);
            self.researching = Some(rx);
            return;
        }
        self.ledger = None;
        let (tx, rx) = channel();
        port.search(&q, self.ask_mode, tx);
        self.pending = Some(rx);
    }

    pub fn poll(&mut self) {
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

impl RagTile {
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
        for line in g.wrap(14.0 * s, &l.summary, Face::Bold, w) {
            g.text(x, y, 14.0 * s, if l.agentic { ACCENT } else { rgb(255, 196, 94) }, &line, Face::Bold);
            y += 20.0 * s;
        }
        y += 6.0 * s;
        // every line of the ledger, wrapped; the question and the summary are shown above, so they are skipped here
        let size = 14.0 * s;
        let lh = 19.0 * s;
        let mut rows: Vec<(String, bool)> = vec![];
        for (i, line) in l.lines.iter().enumerate() {
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

    fn draw_docs(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn RagPort) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 8.0 * s;
        g.text(x, y + 2.0 * s, 20.0 * s, TEXT, &format!("Your documents ({})", self.docs.len()), Face::Bold);
        g.button(x + w - 130.0 * s, y, 13.0 * s, "Back to search", MUTED, Event::Tab(60));
        y += 36.0 * s;
        g.text(x, y, 13.0 * s, MUTED, "These are what the assistant can search. Delete the ones you do not need; samples are marked.", Face::Sans);
        y += 28.0 * s;
        if let Some((m, ok)) = &self.status {
            iyakki_uikit::fx::status_line(g, x, y, w, s, m, *ok);
            y += 24.0 * s;
        }
        if self.docs.is_empty() {
            iyakki_uikit::fx::empty_state(g, R::new(x, y, w, 220.0 * s), s, Icon::Rag, "No documents yet", "Put .txt or .md files in the documents folder and open RAG again.", ACCENT);
        }
        let rh = 56.0 * s;
        let fit = (((area.y + area.h - y - 64.0 * s) / (rh + 6.0 * s)).floor() as usize).max(1);
        let start = self.dsel.saturating_sub(fit - 1);
        for (i, d) in self.docs.iter().enumerate().skip(start).take(fit) {
            let r = R::new(x, y, w, rh);
            iyakki_uikit::fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, i == self.dsel, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &d.title);
            g.rect(r.x, r.y + 10.0 * s, 4.0 * s, r.h - 20.0 * s, if d.ns == "shared" { rgb(120, 170, 255) } else { ACCENT });
            let title = g.fit(16.0 * s, &d.title, Face::Bold, w - 260.0 * s);
            g.text(r.x + 18.0 * s, r.y + 8.0 * s, 16.0 * s, TEXT, &title, Face::Bold);
            let tag = format!("{}{} · {} passage{}", if d.is_sample() { "SAMPLE · " } else { "" }, if d.ns == "shared" { "shared" } else { "mine" }, d.chunks, if d.chunks == 1 { "" } else { "s" });
            g.text(r.x + 18.0 * s, r.y + 32.0 * s, 12.0 * s, if d.is_sample() { rgb(255, 196, 94) } else { MUTED }, &tag, Face::Sans);
            if i == self.dsel && port.can_delete(d) {
                g.button(r.x + r.w - 124.0 * s, r.y + 11.0 * s, 13.0 * s, "Delete (d)", ERR, Event::Tab(61));
            }
            y += rh + 6.0 * s;
        }
        if let Some(d) = &self.confirm {
            render_confirm(g, area, s, &format!("Delete “{}” from your documents?", d.title), "Delete");
        }
    }
}

impl RagTile {
    pub fn title(&self) -> &'static str {
        "RAG"
    }
    pub fn icon(&self) -> Icon {
        Icon::Rag
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        "Enter run · Tab search/ask · Ctrl+O your documents (delete the ones you do not need) · click a result to copy · Esc back".into()
    }

    pub fn handle(&mut self, ev: &Event, port: &dyn RagPort) -> Answer {
        // the Documents list
        if let Some(d) = self.confirm.clone() {
            if matches!(ev, Event::Key(Key::Char('y')) | Event::Key(Key::Enter)) {
                self.confirm = None;
                self.delete_doc(port, &d);
            } else if matches!(ev, Event::Key(_)) {
                self.confirm = None;
            }
            return Answer::Stay;
        }
        match ev {
            Event::Tab(60) | Event::Key(Key::Ctrl('o')) => {
                self.docs_open = !self.docs_open;
                self.status = None;
                if self.docs_open {
                    self.load_docs(port);
                }
                return Answer::Stay;
            }
            Event::Tab(61) if self.docs_open => {
                if let Some(d) = self.docs.get(self.dsel).cloned() {
                    self.confirm = Some(d);
                }
                return Answer::Stay;
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
            return Answer::Stay;
        }
        if matches!(ev, Event::Tab(62) | Event::Key(Key::Ctrl('g'))) && !self.docs_open {
            self.agentic = !self.agentic;
            if self.agentic {
                self.ask_mode = true;
            }
            return Answer::Stay;
        }
        if matches!(ev, Event::Tab(63) | Event::Key(Key::Ctrl('e'))) && !self.docs_open {
            self.open_ledger(port);
            return Answer::Stay;
        }
        if !self.docs_open {
            if self.pane.handle(ev) {
                return Answer::Stay;
            }
            if matches!(ev, Event::Key(Key::Ctrl('a'))) && self.query.value.is_empty() {
                self.pane.select_all();
                return Answer::Stay;
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
                        if port.can_delete(&d) {
                            self.confirm = Some(d);
                        } else {
                            self.status = Some(("Only an administrator can delete shared documents.".into(), false));
                        }
                    }
                }
                _ => {}
            }
            return Answer::Stay;
        }
        match ev {
            Event::Tick => self.poll(),
            Event::Key(Key::Esc) => return Answer::Back,
            Event::Key(Key::Tab) => self.ask_mode = !self.ask_mode,
            Event::Key(Key::Enter) => self.submit(port),
            Event::Paste(t) => {
                self.query.paste(t);
            }
            Event::FocusAt(_, x) => self.query.click(*x),
            // click on a result: copy its text
            Event::Row(i) => {
                if let Some(h) = self.hits.get(*i) {
                    port.copy(&h.text);
                }
            }
            Event::Key(k) => {
                self.query.handle(k);
            }
            _ => {}
        }
        Answer::Stay
    }

    pub fn context_items(&mut self, x: i32, y: i32) -> Vec<iyakki_uikit::ctxmenu::Item> {
        if self.docs_open {
            Vec::new()
        } else {
            self.pane.context_items(x, y)
        }
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn RagPort, tick: u64) {
        self.pane.begin(R::new(0.0, 0.0, 0.0, 0.0));
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 8.0 * s;
        if self.docs_open {
            self.draw_docs(g, area, s, port);
            return;
        }
        if self.ledger_open {
            self.draw_ledger(g, area, s);
            return;
        }
        let mode = if self.ask_mode { if self.agentic { "Ask the model (agentic)" } else { "Ask the model" } } else { "Search" };
        let ext = port.external_backends();
        let info = if self.indexing.is_some() { "indexing documents…".to_string() } else { self.info.clone() };
        let info = format!("{info}{}   mode: {mode}", if ext > 0 { format!("  ·  {ext} external backend(s)") } else { String::new() });
        // the buttons of the top row sit at the right; the status text takes what is left
        let docs_x = x + w - 120.0 * s;
        let bsize = 13.0 * s;
        let bw_of = |g: &mut Gfx, label: &str| g.text_width(bsize, label, Face::Bold) + bsize * 1.7;
        let a_label = if self.agentic { "Agentic: on" } else { "Agentic: off" };
        // in a narrow window the three buttons would run out of the left edge: they go on a row of their own below the status line
        let narrow = {
            let (aw, ew, dw) = (bw_of(g, a_label), bw_of(g, "Evidence"), bw_of(g, "Documents"));
            aw + ew + dw + 16.0 * s + 160.0 * s > w
        };
        if narrow {
            let shown = g.fit(15.0 * s, &info, Face::Sans, w);
            g.text(x, y, 15.0 * s, MUTED, &shown, Face::Sans);
            y += 30.0 * s;
            let mut bx = x;
            for (label, col, ev) in [("Documents", MUTED, Event::Tab(60)), ("Evidence", MUTED, Event::Tab(63)), (a_label, if self.agentic { ACCENT } else { MUTED }, Event::Tab(62))] {
                // a button that does not fit the row goes to the next one
                if bx > x && bx + bw_of(g, label) > x + w {
                    bx = x;
                    y += 36.0 * s;
                }
                bx += g.button(bx, y, bsize, label, col, ev) + 8.0 * s;
            }
            y += 36.0 * s;
        } else {
            let mut left = docs_x - 8.0 * s;
            let aw = bw_of(g, a_label);
            left -= aw;
            g.button(left, y, bsize, a_label, if self.agentic { ACCENT } else { MUTED }, Event::Tab(62));
            let ew = bw_of(g, "Evidence");
            left -= ew + 8.0 * s;
            g.button(left, y, bsize, "Evidence", MUTED, Event::Tab(63));
            let shown = g.fit(15.0 * s, &info, Face::Sans, (left - 8.0 * s - x).max(40.0 * s));
            g.text(x, y, 15.0 * s, MUTED, &shown, Face::Sans);
            y += 28.0 * s;
        }
        let bw = 120.0 * s;
        self.query.render(g, R::new(x, y, w - bw - 12.0 * s - 116.0 * s, 46.0 * s), true, s, blink(tick), Face::Sans, 0);
        if !narrow {
            g.button(docs_x, y - 28.0 * s, 13.0 * s, "Documents", MUTED, Event::Tab(60));
        }
        let (bx, by) = (x + w - bw - 116.0 * s, y + 6.0 * s);
        g.button(bx, by, 15.0 * s, if self.ask_mode { "Ask" } else { "Search" }, ACCENT, Event::Key(Key::Enter));
        g.button(bx + 112.0 * s, by, 15.0 * s, if self.ask_mode { "⇄ Search" } else { "⇄ Ask" }, MUTED, Event::Key(Key::Tab));
        y += 62.0 * s;
        if self.pending.is_some() || self.researching.is_some() {
            let word = if self.researching.is_some() { "researching" } else if self.ask_mode { "thinking" } else { "searching" };
            iyakki_uikit::fx::chip(g, x, y - 2.0 * s, s, &format!("{word}{}", ".".repeat(1 + (tick / 10 % 3) as usize)), iyakki_uikit::fx::CYAN, false);
            y += 30.0 * s;
        }
        // the budget used and why the run stopped are always shown
        if let Some(l) = &self.ledger {
            let line = g.fit(13.0 * s, &l.summary, Face::Sans, w);
            g.text(x, y, 13.0 * s, if l.agentic { ACCENT } else { rgb(255, 196, 94) }, &line, Face::Sans);
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
            iyakki_uikit::fx::card(g, R::new(x, y, w, h), s, ACCENT, 0.4);
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
            iyakki_uikit::fx::row_at(g, x, y, w, ch, 12.0 * s, false, g.hovered(R::new(x, y, w, ch)), ACCENT);
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
            let dirs = port.docs_folders();
            iyakki_uikit::fx::empty_state(g, R::new(x, y, w, (area.y + area.h - y).max(160.0 * s)), s, Icon::Rag, "Ask your documents", &format!("Type a question or keywords. Documents come from {dirs} (.txt/.md/…) and from external backends enabled in Settings."), ACCENT);
        }
    }
}

#[cfg(test)]
mod tests;
