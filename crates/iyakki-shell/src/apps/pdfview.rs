//! The PDF viewer: pages, zoom, fit width, text search with highlights and a result count, copy text, a page list and the form
//! fields shown read-only. It is a component ([`PdfView`]) that the PDF tile ([`PdfApp`]) and the browser's document handling
//! both host. All parsing and drawing of the file happens on a worker thread that opens its own copy of the bytes (the reader
//! library is single-threaded by design), so a big or hostile file never stalls the screen. What the reader cannot show is listed
//! under the page, never hidden (docs/PDF.md).

use crate::safeclamp::SafeClamp;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;

use iyakki_pdf::{BoxGlyphs, Document, FontStyle, GlyphBitmap, GlyphSource};

use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

pub const ACCENT: [u8; 4] = rgb(255, 150, 120);
/// Click ids of the viewer's buttons: `Event::Tab(TAB_BASE + n)`.
pub const TAB_BASE: usize = 9300;
const T_PREV: usize = 0;
const T_NEXT: usize = 1;
const T_ZOUT: usize = 2;
const T_ZIN: usize = 3;
const T_FIT: usize = 4;
const T_SEARCH: usize = 5;
const T_COPY: usize = 6;
const T_FIELDS: usize = 7;
const T_HPREV: usize = 8;
const T_HNEXT: usize = 9;
const T_CLOSE: usize = 10;
const T_ROW: usize = 100;
const SEARCH_FIELD: usize = 9399;
const ZOOMS: [f32; 8] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];
/// Pixels per point at 100% (96 dpi).
const PX_PT: f32 = 96.0 / 72.0;

pub fn is_tab(n: usize) -> bool {
    (TAB_BASE..TAB_BASE + 1000).contains(&n) || n == SEARCH_FIELD
}

// ---- fonts for the reader library, from the shell's own ----------------------------------------------------------------

struct ShellGlyphs {
    g: RefCell<Gfx>,
}

impl GlyphSource for ShellGlyphs {
    fn glyph(&self, ch: char, style: FontStyle, px: f32) -> Option<GlyphBitmap> {
        if ch.is_whitespace() {
            return Some(GlyphBitmap { left: 0, top: 0, w: 0, h: 0, alpha: vec![] });
        }
        let face = if style.mono {
            Face::Mono
        } else if style.bold {
            Face::Bold
        } else {
            Face::Sans
        };
        let m = self.g.borrow_mut().text_mask(px, &ch.to_string(), face);
        if m.w == 0 || m.h == 0 {
            return None;
        }
        Some(GlyphBitmap { left: m.x, top: m.y, w: m.w, h: m.h, alpha: m.alpha })
    }
}

// ---- the worker ----------------------------------------------------------------------------------------------------------

enum Job {
    Render { gen: u64, page: usize, scale: f64 },
    Search { gen: u64, query: String },
    Text { page: usize },
    AllText,
}

struct Opened {
    pages: usize,
    sizes: Vec<(f32, f32)>,
    title: String,
    author: String,
    fields: Vec<(String, String, String)>,
    repaired: bool,
}

enum Done {
    Opened(Opened),
    Failed(String),
    Rendered { gen: u64, page: usize, w: u32, h: u32, rgba: Vec<u8>, matrix: [f64; 6], notes: Vec<String> },
    Hits { gen: u64, page: usize, hits: Vec<iyakki_pdf::Hit> },
    SearchDone { gen: u64 },
    Text { page: Option<usize>, text: String },
}

static SEARCH_GEN: AtomicU64 = AtomicU64::new(0);

fn worker(bytes: Arc<Vec<u8>>, rx: Receiver<Job>, tx: Sender<Done>, latest_search: Arc<AtomicU64>) {
    let doc = match Document::open((*bytes).clone()) {
        Ok(d) => d,
        Err(e) => {
            let _ = tx.send(Done::Failed(e.to_string()));
            return;
        }
    };
    let info = doc.info();
    let sizes: Vec<(f32, f32)> = (0..doc.page_count().min(50_000)).map(|i| doc.page_size(i).map(|(w, h)| (w as f32, h as f32)).unwrap_or((612.0, 792.0))).collect();
    let fields = doc.form_fields().into_iter().map(|f| (f.name, f.kind, f.value)).collect();
    let _ = tx.send(Done::Opened(Opened { pages: doc.page_count(), sizes, title: info.title, author: info.author, fields, repaired: info.repaired }));
    let glyphs = ShellGlyphs { g: RefCell::new(Gfx::new(64, 64)) };
    let mut queue: Vec<Job> = Vec::new();
    loop {
        if queue.is_empty() {
            match rx.recv() {
                Ok(j) => queue.push(j),
                Err(_) => return,
            }
        }
        while let Ok(j) = rx.try_recv() {
            queue.push(j);
        }
        // only the newest render matters
        let last_render = queue.iter().rposition(|j| matches!(j, Job::Render { .. }));
        let mut jobs: Vec<Job> = Vec::new();
        for (i, j) in queue.drain(..).enumerate() {
            if matches!(j, Job::Render { .. }) && Some(i) != last_render {
                continue;
            }
            jobs.push(j);
        }
        for j in jobs {
            match j {
                Job::Render { gen, page, scale } => {
                    let g: &dyn GlyphSource = &glyphs;
                    match doc.render(page, scale, g) {
                        Ok(r) => {
                            let m = doc.page_matrix(page, scale).map(|m| m.0).unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
                            let _ = tx.send(Done::Rendered { gen, page, w: r.width, h: r.height, rgba: r.rgba, matrix: m, notes: r.unsupported });
                        }
                        Err(e) => {
                            let _ = tx.send(Done::Failed(format!("page {}: {e}", page + 1)));
                        }
                    }
                }
                Job::Search { gen, query } => {
                    for p in 0..doc.page_count().min(3000) {
                        if latest_search.load(Ordering::Relaxed) != gen {
                            break;
                        }
                        if let Ok(h) = doc.search_page(p, &query, false) {
                            if !h.is_empty() {
                                let _ = tx.send(Done::Hits { gen, page: p, hits: h });
                            }
                        }
                    }
                    let _ = tx.send(Done::SearchDone { gen });
                }
                Job::Text { page } => {
                    let _ = tx.send(Done::Text { page: Some(page), text: doc.page_text(page).unwrap_or_default() });
                }
                Job::AllText => {
                    let _ = tx.send(Done::Text { page: None, text: doc.text(1_000_000) });
                }
            }
        }
    }
}

// ---- the view -------------------------------------------------------------------------------------------------------------

struct Shown {
    page: usize,
    key: i64,
    pm: Arc<tiny_skia::Pixmap>,
    matrix: [f64; 6],
    notes: Vec<String>,
}

pub struct PdfView {
    pub name: String,
    bytes: Option<Arc<Vec<u8>>>,
    tx: Option<Sender<Job>>,
    rx: Option<Receiver<Done>>,
    pub error: Option<String>,
    pub pages: usize,
    sizes: Vec<(f32, f32)>,
    pub title: String,
    pub author: String,
    pub fields: Vec<(String, String, String)>,
    pub repaired: bool,
    opened: bool,
    pub page: usize,
    pub zoom: f32,
    pub fit_width: bool,
    scroll: f32,
    shown: Option<Shown>,
    asked: Option<(usize, i64)>,
    gen: u64,
    pub search_open: bool,
    search: TextField,
    search_focus: bool,
    pub query: String,
    pub hits: Vec<(usize, iyakki_pdf::Hit)>,
    pub hit: usize,
    pub searching: bool,
    search_gen: u64,
    latest_search: Arc<AtomicU64>,
    focus_hit: bool,
    fields_open: bool,
    sidebar_scroll: usize,
    pub status: Option<String>,
    last_view: (f32, f32),
    view_rect: R,
    dirty: bool,
}

impl Default for PdfView {
    fn default() -> Self {
        PdfView::empty()
    }
}

impl PdfView {
    pub fn empty() -> PdfView {
        PdfView {
            name: String::new(),
            bytes: None,
            tx: None,
            rx: None,
            error: None,
            pages: 0,
            sizes: vec![],
            title: String::new(),
            author: String::new(),
            fields: vec![],
            repaired: false,
            opened: false,
            page: 0,
            zoom: 1.0,
            fit_width: true,
            scroll: 0.0,
            shown: None,
            asked: None,
            gen: 0,
            search_open: false,
            search: TextField::new("Find in this PDF", false),
            search_focus: false,
            query: String::new(),
            hits: vec![],
            hit: 0,
            searching: false,
            search_gen: 0,
            latest_search: Arc::new(AtomicU64::new(0)),
            focus_hit: false,
            fields_open: false,
            sidebar_scroll: 0,
            status: None,
            last_view: (0.0, 0.0),
            view_rect: R::new(0.0, 0.0, 0.0, 0.0),
            dirty: false,
        }
    }

    pub fn open_bytes(name: &str, bytes: Vec<u8>) -> PdfView {
        let mut v = PdfView::empty();
        v.name = name.to_string();
        let bytes = Arc::new(bytes);
        let (jtx, jrx) = channel();
        let (dtx, drx) = channel();
        let b2 = bytes.clone();
        let latest = v.latest_search.clone();
        match std::thread::Builder::new().name("iyakki-pdf".into()).stack_size(32 << 20).spawn(move || worker(b2, jrx, dtx, latest)) {
            Ok(_) => {
                v.bytes = Some(bytes);
                v.tx = Some(jtx);
                v.rx = Some(drx);
            }
            Err(e) => v.error = Some(format!("the viewer could not start: {e}")),
        }
        v
    }

    /// Open a file the person may read (the caller checked the file rules).
    pub fn open_file(path: &std::path::Path) -> PdfView {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "PDF".into());
        match std::fs::metadata(path) {
            Ok(m) if m.len() > iyakki_pdf::file::MAX_FILE as u64 => {
                let mut v = PdfView::empty();
                v.name = name;
                v.error = Some("this PDF is too large to open here".into());
                v
            }
            Ok(_) => match std::fs::read(path) {
                Ok(b) => PdfView::open_bytes(&name, b),
                Err(e) => {
                    let mut v = PdfView::empty();
                    v.name = name;
                    v.error = Some(format!("{}: {e}", path.display()));
                    v
                }
            },
            Err(e) => {
                let mut v = PdfView::empty();
                v.name = name;
                v.error = Some(format!("{}: {e}", path.display()));
                v
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.bytes.is_some() || self.error.is_some()
    }

    fn send(&self, j: Job) {
        if let Some(t) = &self.tx {
            let _ = t.send(j);
        }
    }

    /// Pixels per point for the current zoom in a view `w` wide.
    fn scale_for(&self, view_w: f32, ui: f32) -> f32 {
        if self.fit_width {
            if let Some(&(pw, _)) = self.sizes.get(self.page) {
                return ((view_w - 28.0 * ui) / pw.max(1.0)).clamp(0.1, 6.0);
            }
        }
        self.zoom * PX_PT * ui
    }

    fn goto(&mut self, p: usize) {
        if self.pages == 0 {
            return;
        }
        let p = p.min(self.pages - 1);
        if p != self.page {
            self.page = p;
            self.scroll = 0.0;
        }
        self.dirty = true;
    }

    pub fn page_text_request(&mut self) {
        self.send(Job::Text { page: self.page });
        self.status = Some("Copying…".into());
    }

    fn start_search(&mut self) {
        let q = self.search.value.trim().to_string();
        self.query = q.clone();
        self.hits.clear();
        self.hit = 0;
        self.search_gen = SEARCH_GEN.fetch_add(1, Ordering::Relaxed) + 1;
        self.latest_search.store(self.search_gen, Ordering::Relaxed);
        if q.is_empty() {
            self.searching = false;
            return;
        }
        self.searching = true;
        self.send(Job::Search { gen: self.search_gen, query: q });
    }

    fn step_hit(&mut self, back: bool) {
        if self.hits.is_empty() {
            return;
        }
        self.hit = if back { (self.hit + self.hits.len() - 1) % self.hits.len() } else { (self.hit + 1) % self.hits.len() };
        let p = self.hits[self.hit].0;
        self.goto(p);
        self.focus_hit = true;
    }

    fn zoom_step(&mut self, up: bool) {
        let cur = if self.fit_width { self.last_scale() / PX_PT } else { self.zoom };
        let next = if up { ZOOMS.iter().copied().find(|z| *z > cur + 0.01).unwrap_or(4.0) } else { ZOOMS.iter().rev().copied().find(|z| *z < cur - 0.01).unwrap_or(0.5) };
        self.zoom = next;
        self.fit_width = false;
        self.dirty = true;
    }

    fn last_scale(&self) -> f32 {
        self.shown.as_ref().map(|s| s.matrix[0].abs().max(s.matrix[1].abs()) as f32).unwrap_or(PX_PT)
    }

    /// Poll the worker: true when something on screen changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            let Some(rx) = &self.rx else { break };
            match rx.try_recv() {
                Ok(Done::Opened(o)) => {
                    self.opened = true;
                    self.pages = o.pages;
                    self.sizes = o.sizes;
                    self.title = o.title;
                    self.author = o.author;
                    self.fields = o.fields;
                    self.repaired = o.repaired;
                    self.dirty = true;
                    changed = true;
                }
                Ok(Done::Failed(e)) => {
                    if !self.opened {
                        self.error = Some(e);
                        self.rx = None;
                        self.tx = None;
                    } else {
                        self.status = Some(e);
                    }
                    changed = true;
                    if self.rx.is_none() {
                        break;
                    }
                }
                Ok(Done::Rendered { gen, page, w, h, rgba, matrix, notes }) => {
                    changed = true;
                    if gen != self.gen {
                        continue;
                    }
                    if let Ok(pm) = super::browser_images::bitmap_pixmap(w, h, &rgba) {
                        let key = self.asked.map(|a| a.1).unwrap_or(0);
                        self.shown = Some(Shown { page, key, pm: Arc::new(pm), matrix, notes });
                        if self.focus_hit {
                            self.focus_hit = false;
                            self.scroll_to_hit();
                        }
                    }
                }
                Ok(Done::Hits { gen, page, hits }) => {
                    if gen == self.search_gen {
                        self.hits.extend(hits.into_iter().map(|h| (page, h)));
                        if self.hits.len() == 1 {
                            self.hit = 0;
                            let p = self.hits[0].0;
                            self.goto(p);
                            self.focus_hit = true;
                        }
                        changed = true;
                    }
                }
                Ok(Done::SearchDone { gen }) => {
                    if gen == self.search_gen {
                        self.searching = false;
                        changed = true;
                    }
                }
                Ok(Done::Text { page, text }) => {
                    let words = text.split_whitespace().count();
                    if text.trim().is_empty() {
                        self.status = Some(match page {
                            Some(_) => "This page has no text to copy (a scanned page is a picture; there is no text recognition yet).".into(),
                            None => "This PDF has no text to copy.".into(),
                        });
                    } else {
                        crate::ui::clipboard::copy(&text);
                        self.status = Some(match page {
                            Some(p) => format!("Copied the text of page {} ({words} words).", p + 1),
                            None => format!("Copied all the text ({words} words)."),
                        });
                    }
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    break;
                }
            }
        }
        changed
    }

    fn scroll_to_hit(&mut self) {
        let (Some(s), Some((p, h))) = (&self.shown, self.hits.get(self.hit)) else { return };
        if s.page != *p {
            return;
        }
        let r = h.rects[0];
        let m = &s.matrix;
        let y = (m[1] * r[0] + m[3] * r[1] + m[5]).min(m[1] * r[2] + m[3] * r[3] + m[5]) as f32;
        self.scroll = (y - self.view_rect.h * 0.35).max(0.0);
    }

    // ---- input ---------------------------------------------------------------------------------------------------------------

    pub fn handle(&mut self, ev: &Event) -> Flow {
        match ev {
            Event::Tick => {
                self.poll();
            }
            Event::Tab(n) if is_tab(*n) && *n != SEARCH_FIELD => self.click(*n - TAB_BASE),
            Event::Tab(n) if *n == SEARCH_FIELD => self.search_focus = true,
            Event::Focus(id) | Event::FocusAt(id, _) if *id == SEARCH_FIELD => {
                self.search_open = true;
                self.search_focus = true;
            }
            Event::Scroll(n) => {
                self.scroll = (self.scroll - *n as f32 * 60.0).max(0.0);
                self.clamp_scroll_page_turn(*n);
            }
            Event::ScrollPx(n) => self.scroll = (self.scroll - *n as f32).max(0.0),
            Event::Paste(t) if self.search_focus => {
                for c in t.chars().filter(|c| !c.is_control()) {
                    self.search.handle(&Key::Char(c));
                }
            }
            Event::Key(k) => return self.key(k),
            _ => {}
        }
        Flow::Stay
    }

    fn clamp_scroll_page_turn(&mut self, n: i32) {
        // wheel past the end of the page turns it
        if let Some(s) = &self.shown {
            let max = (s.pm.height() as f32 - self.view_rect.h).max(0.0);
            if n < 0 && self.scroll >= max + 120.0 && self.page + 1 < self.pages {
                self.goto(self.page + 1);
            } else if n > 0 && self.scroll <= 0.0 && self.page > 0 && self.last_view.1 > 0.0 {
                // stays: going back a page by wheel would jump the person around
            } else if self.scroll > max + 120.0 {
                self.scroll = max + 120.0;
            }
        }
    }

    fn click(&mut self, n: usize) {
        match n {
            T_PREV => self.goto(self.page.saturating_sub(1)),
            T_NEXT => self.goto(self.page + 1),
            T_ZOUT => self.zoom_step(false),
            T_ZIN => self.zoom_step(true),
            T_FIT => {
                self.fit_width = true;
                self.dirty = true;
            }
            T_SEARCH => {
                self.search_open = !self.search_open;
                self.search_focus = self.search_open;
            }
            T_COPY => self.page_text_request(),
            T_FIELDS => self.fields_open = !self.fields_open,
            T_HPREV => self.step_hit(true),
            T_HNEXT => self.step_hit(false),
            T_CLOSE => {}
            n if n >= T_ROW => self.goto(self.sidebar_scroll + n - T_ROW),
            _ => {}
        }
    }

    fn key(&mut self, k: &Key) -> Flow {
        if self.search_focus {
            match k {
                Key::Enter => {
                    if self.search.value.trim() != self.query {
                        self.start_search();
                    } else {
                        self.step_hit(false);
                    }
                    return Flow::Stay;
                }
                Key::Esc => {
                    self.search_focus = false;
                    self.search_open = false;
                    return Flow::Stay;
                }
                Key::Tab => {
                    self.search_focus = false;
                    return Flow::Stay;
                }
                Key::Ctrl('f') | Key::Ctrl('F') => return Flow::Stay,
                _ => {
                    self.search.handle(k);
                    return Flow::Stay;
                }
            }
        }
        match k {
            Key::Esc => {
                if self.search_open {
                    self.search_open = false;
                    return Flow::Stay;
                }
                return Flow::Back;
            }
            Key::Char('/') | Key::Ctrl('f') | Key::Ctrl('F') => {
                self.search_open = true;
                self.search_focus = true;
            }
            Key::Char('n') | Key::Right => self.goto(self.page + 1),
            Key::Char('p') | Key::Left => self.goto(self.page.saturating_sub(1)),
            Key::PageDown | Key::Char(' ') => {
                let step = self.view_rect.h * 0.9;
                let max = self.shown.as_ref().map_or(0.0, |s| (s.pm.height() as f32 - self.view_rect.h).max(0.0));
                if self.scroll + 4.0 >= max && self.page + 1 < self.pages {
                    self.goto(self.page + 1);
                } else {
                    self.scroll = (self.scroll + step).min(max);
                }
            }
            Key::PageUp => {
                if self.scroll <= 0.5 && self.page > 0 {
                    self.goto(self.page - 1);
                } else {
                    self.scroll = (self.scroll - self.view_rect.h * 0.9).max(0.0);
                }
            }
            Key::Down => self.scroll += 60.0,
            Key::Up => self.scroll = (self.scroll - 60.0).max(0.0),
            Key::Home => self.goto(0),
            Key::End => self.goto(self.pages.saturating_sub(1)),
            Key::Char('+') | Key::Char('=') => self.zoom_step(true),
            Key::Char('-') => self.zoom_step(false),
            Key::Char('w') => {
                self.fit_width = true;
                self.dirty = true;
            }
            Key::Char('c') | Key::Ctrl('c') => self.page_text_request(),
            Key::Char('C') => {
                self.send(Job::AllText);
                self.status = Some("Copying…".into());
            }
            Key::Char('f') => self.fields_open = !self.fields_open,
            Key::Char('g') if self.search_open => self.step_hit(false),
            Key::Char('G') if self.search_open => self.step_hit(true),
            _ => {}
        }
        Flow::Stay
    }

    pub fn hint() -> &'static str {
        "n/p page · + - zoom · w fit width · / search · g next match · c copy text · Esc back"
    }

    // ---- drawing -------------------------------------------------------------------------------------------------------------

    fn button(&self, g: &mut Gfx, x: &mut f32, y: f32, s: f32, label: &str, id: usize, on: bool) {
        if on {
            let w = g.button(*x, y, 12.5 * s, label, ACCENT, Event::Tab(TAB_BASE + id));
            *x += w + 6.0 * s;
        } else {
            let w = g.button_off(*x, y, 12.5 * s, label);
            *x += w + 6.0 * s;
        }
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, _tick: u64) {
        self.poll();
        if let Some(e) = &self.error {
            g.text(area.x + 20.0 * s, area.y + 24.0 * s, 15.0 * s, ERR, &format!("{}: {e}", if self.name.is_empty() { "PDF" } else { &self.name }), Face::Bold);
            return;
        }
        if !self.opened {
            g.text(area.x + 20.0 * s, area.y + 24.0 * s, 15.0 * s, MUTED, &format!("Opening {}…", self.name), Face::Sans);
            return;
        }
        // toolbar
        let bar_h = 38.0 * s;
        let mut x = area.x + 8.0 * s;
        let y = area.y + 5.0 * s;
        self.button(g, &mut x, y, s, "Previous", T_PREV, self.page > 0);
        self.button(g, &mut x, y, s, "Next", T_NEXT, self.page + 1 < self.pages);
        g.text(x, y + 6.0 * s, 13.0 * s, TEXT, &format!("Page {} / {}", self.page + 1, self.pages), Face::Bold);
        x += g.text_width(13.0 * s, &format!("Page {} / {}", self.page + 1, self.pages), Face::Bold) + 14.0 * s;
        self.button(g, &mut x, y, s, "-", T_ZOUT, true);
        self.button(g, &mut x, y, s, "+", T_ZIN, true);
        self.button(g, &mut x, y, s, "Fit width", T_FIT, !self.fit_width);
        self.button(g, &mut x, y, s, "Search", T_SEARCH, true);
        self.button(g, &mut x, y, s, "Copy text", T_COPY, true);
        if !self.fields.is_empty() {
            self.button(g, &mut x, y, s, &format!("Form ({})", self.fields.len()), T_FIELDS, true);
        }
        let mut top = area.y + bar_h;
        if self.search_open {
            let fr = R::new(area.x + 8.0 * s, top + 2.0 * s, 260.0 * s, 28.0 * s);
            self.search.render(g, fr, self.search_focus, s, true, Face::Sans, SEARCH_FIELD);
            g.hit(fr, Event::Focus(SEARCH_FIELD), "Find in this PDF");
            let mut bx = fr.x + fr.w + 8.0 * s;
            self.button(g, &mut bx, top + 1.0 * s, s, "Prev match", T_HPREV, !self.hits.is_empty());
            self.button(g, &mut bx, top + 1.0 * s, s, "Next match", T_HNEXT, !self.hits.is_empty());
            let msg = if self.query.is_empty() {
                "Type, then Enter".to_string()
            } else if self.searching && self.hits.is_empty() {
                "Searching…".to_string()
            } else if self.hits.is_empty() {
                "No matches".to_string()
            } else {
                format!("{} of {}{}", self.hit + 1, self.hits.len(), if self.searching { "+ (searching)" } else { "" })
            };
            g.text(bx + 4.0 * s, top + 8.0 * s, 13.0 * s, if self.hits.is_empty() && !self.query.is_empty() && !self.searching { ERR } else { MUTED }, &msg, Face::Sans);
            top += 34.0 * s;
        }
        let foot_h = 26.0 * s;
        let body = R::new(area.x, top, area.w, (area.y + area.h - top - foot_h).max(10.0));
        // sidebar: the page list (or the form fields)
        let side_w = if area.w >= 620.0 * s { 150.0 * s } else { 0.0 };
        if side_w > 0.0 {
            self.draw_sidebar(g, R::new(body.x, body.y, side_w, body.h), s);
        }
        let view = R::new(body.x + side_w, body.y, body.w - side_w, body.h);
        self.view_rect = view;
        self.draw_page(g, view, s);
        // footer
        let mut msg = self.status.clone().unwrap_or_default();
        if msg.is_empty() {
            let mut parts = vec![];
            if !self.title.is_empty() {
                parts.push(self.title.clone());
            }
            if !self.author.is_empty() {
                parts.push(self.author.clone());
            }
            if self.repaired {
                parts.push("damaged file, rebuilt while opening".into());
            }
            if let Some(sh) = &self.shown {
                if !sh.notes.is_empty() {
                    parts.push(format!("not shown on this page: {}", sh.notes.join("; ")));
                }
            }
            if !self.fields.is_empty() {
                parts.push(format!("{} form field(s), read-only", self.fields.len()));
            }
            msg = parts.join(" · ");
        }
        let fit = g.fit(12.0 * s, &msg, Face::Sans, area.w - 16.0 * s);
        g.text(area.x + 8.0 * s, area.y + area.h - foot_h + 5.0 * s, 12.0 * s, DIM, &fit, Face::Sans);
    }

    fn draw_sidebar(&mut self, g: &mut Gfx, r: R, s: f32) {
        g.rect(r.x, r.y, r.w, r.h, rgb(18, 22, 38));
        if self.fields_open && !self.fields.is_empty() {
            g.text(r.x + 8.0 * s, r.y + 6.0 * s, 12.0 * s, MUTED, "Form fields (read-only)", Face::Bold);
            let mut y = r.y + 26.0 * s;
            for (n, k, v) in self.fields.iter().take(40) {
                let line = g.fit(11.5 * s, &format!("{n} ({k}): {v}"), Face::Sans, r.w - 12.0 * s);
                g.text(r.x + 8.0 * s, y, 11.5 * s, TEXT, &line, Face::Sans);
                y += 16.0 * s;
                if y > r.y + r.h - 16.0 * s {
                    break;
                }
            }
            return;
        }
        let row_h = 26.0 * s;
        let rows = ((r.h / row_h) as usize).max(1);
        if self.page < self.sidebar_scroll {
            self.sidebar_scroll = self.page;
        }
        if self.page >= self.sidebar_scroll + rows {
            self.sidebar_scroll = self.page + 1 - rows;
        }
        for i in 0..rows {
            let p = self.sidebar_scroll + i;
            if p >= self.pages {
                break;
            }
            let rr = R::new(r.x + 4.0 * s, r.y + 2.0 * s + i as f32 * row_h, r.w - 8.0 * s, row_h - 3.0 * s);
            if p == self.page {
                g.rrect(rr.x, rr.y, rr.w, rr.h, 5.0 * s, rgb(52, 40, 50));
            }
            let n = self.hits.iter().filter(|(hp, _)| *hp == p).count();
            let label = if n > 0 { format!("Page {} · {} match{}", p + 1, n, if n == 1 { "" } else { "es" }) } else { format!("Page {}", p + 1) };
            let fit = g.fit(12.0 * s, &label, Face::Sans, rr.w - 10.0 * s);
            g.text(rr.x + 6.0 * s, rr.y + 4.0 * s, 12.0 * s, if p == self.page { TEXT } else { MUTED }, &fit, Face::Sans);
            g.hit(rr, Event::Tab(TAB_BASE + T_ROW + i), &format!("Page {}", p + 1));
        }
    }

    fn draw_page(&mut self, g: &mut Gfx, view: R, s: f32) {
        g.rect(view.x, view.y, view.w, view.h, rgb(34, 36, 44));
        if self.pages == 0 {
            return;
        }
        let scale = self.scale_for(view.w, s);
        let key = (scale * 1000.0).round() as i64;
        let want = (self.page, key);
        if self.last_view != (view.w, view.h) {
            self.last_view = (view.w, view.h);
        }
        if self.asked != Some(want) || self.dirty {
            self.dirty = false;
            self.asked = Some(want);
            self.gen += 1;
            self.send(Job::Render { gen: self.gen, page: self.page, scale: scale as f64 });
        }
        let Some(sh) = &self.shown else {
            g.text(view.x + 16.0 * s, view.y + 16.0 * s, 13.0 * s, MUTED, "Drawing the page…", Face::Sans);
            return;
        };
        let (pw, ph) = (sh.pm.width() as f32, sh.pm.height() as f32);
        let max_scroll = (ph - view.h + 20.0 * s).max(0.0);
        self.scroll = self.scroll.clamp_safe(0.0, max_scroll + 120.0);
        let sc = self.scroll.min(max_scroll);
        let ox = view.x + ((view.w - pw) / 2.0).max(8.0 * s);
        let oy = view.y + 10.0 * s - sc;
        let stale = sh.page != self.page || sh.key != key;
        let (hits_here, cur): (Vec<[f64; 4]>, usize) = {
            let cur_hit = self.hits.get(self.hit).filter(|(p, _)| *p == sh.page).map(|(_, h)| h.rects[0]);
            let all: Vec<[f64; 4]> = self.hits.iter().filter(|(p, _)| *p == sh.page).map(|(_, h)| h.rects[0]).collect();
            let idx = cur_hit.and_then(|c| all.iter().position(|a| *a == c)).unwrap_or(usize::MAX);
            (all, idx)
        };
        let pm = sh.pm.clone();
        let m = sh.matrix;
        g.clipped(view, |g| {
            g.rect(ox + 3.0 * s, oy + 3.0 * s, pw, ph, rgb(12, 13, 18));
            let paint = tiny_skia::PixmapPaint { opacity: if stale { 0.6 } else { 1.0 }, quality: tiny_skia::FilterQuality::Nearest, ..tiny_skia::PixmapPaint::default() };
            g.pm.draw_pixmap(ox.round() as i32, oy.round() as i32, (*pm).as_ref(), &paint, tiny_skia::Transform::identity(), None);
            if !stale {
                for (i, r) in hits_here.iter().enumerate() {
                    let pts = [(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])].map(|(x, y)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]));
                    let (x0, x1) = (pts.iter().map(|p| p.0).fold(f64::MAX, f64::min) as f32, pts.iter().map(|p| p.0).fold(f64::MIN, f64::max) as f32);
                    let (y0, y1) = (pts.iter().map(|p| p.1).fold(f64::MAX, f64::min) as f32, pts.iter().map(|p| p.1).fold(f64::MIN, f64::max) as f32);
                    let col = if i == cur { [255, 140, 40, 130] } else { [255, 235, 60, 100] };
                    g.rect(ox + x0, oy + y0, (x1 - x0).max(3.0), y1 - y0, col);
                }
            }
        });
    }

    /// For tests: the page number being shown and whether its picture has arrived.
    pub fn shown_page(&self) -> Option<usize> {
        self.shown.as_ref().filter(|s| s.page == self.page).map(|s| s.page)
    }

    /// The pixels last drawn for the current page (tests).
    pub fn shown_pixels(&self) -> Option<(u32, u32, Vec<u8>)> {
        self.shown.as_ref().map(|s| (s.pm.width(), s.pm.height(), s.pm.data().to_vec()))
    }

    pub fn scroll_pos(&self) -> f32 {
        self.scroll
    }
}

/// The tile: the viewer, on its own.
pub struct PdfApp {
    pub view: PdfView,
    list: Vec<PathBuf>,
}

impl PdfApp {
    pub fn new(cx: &AppCx) -> PdfApp {
        let mut list = Vec::new();
        if let Ok(dir) = super::browser::BrowserApp::downloads_dir(cx) {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten().take(500) {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
                        list.push(p);
                    }
                }
            }
        }
        list.sort();
        PdfApp { view: PdfView::empty(), list }
    }

    /// Open a file after the file rules agree.
    pub fn open_path(&mut self, path: &std::path::Path, cx: &AppCx) {
        match iyakki_agent::scope::check_read(&cx.session.policy, &cx.session.user, path) {
            Ok(()) => {
                self.view = PdfView::open_file(path);
                cx.audit.record(&cx.session.user, "pdf.open", &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            }
            Err(e) => {
                self.view = PdfView::empty();
                self.view.error = Some(e);
            }
        }
    }
}

impl App for PdfApp {
    fn title(&self) -> &'static str {
        "PDF"
    }
    fn icon(&self) -> Icon {
        Icon::Data
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        PdfView::hint().to_string()
    }
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        if let Some(p) = item.strip_prefix("file:") {
            self.open_path(std::path::Path::new(p), cx);
        }
    }
    fn wants_redraw(&mut self) -> bool {
        self.view.poll()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if !self.view.is_open() {
            match ev {
                Event::Row(i) => {
                    if let Some(p) = self.list.get(*i).cloned() {
                        self.open_path(&p, cx);
                    }
                }
                Event::Key(Key::Esc) => return Flow::Back,
                _ => {}
            }
            return Flow::Stay;
        }
        match self.view.handle(ev) {
            Flow::Back => Flow::Back,
            Flow::Stay => Flow::Stay,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, tick: u64) {
        if self.view.is_open() {
            self.view.render(g, area, s, tick);
            return;
        }
        g.text(area.x + 20.0 * s, area.y + 18.0 * s, 17.0 * s, TEXT, "PDF viewer", Face::Bold);
        let msg = "Open a PDF from the list, from Downloads in the browser (press O on a PDF page), or ask the assistant to open one.";
        let lines = g.wrap(13.0 * s, msg, Face::Sans, area.w - 40.0 * s);
        for (i, l) in lines.iter().enumerate() {
            g.text(area.x + 20.0 * s, area.y + 48.0 * s + i as f32 * 18.0 * s, 13.0 * s, MUTED, l, Face::Sans);
        }
        // the list starts under the last line of the message, however many lines it wraps to on a narrow window
        let mut y = (area.y + 100.0 * s).max(area.y + 48.0 * s + lines.len() as f32 * 18.0 * s + 12.0 * s);
        if self.list.is_empty() {
            g.text(area.x + 20.0 * s, y, 13.0 * s, DIM, "No PDF files in Downloads yet.", Face::Sans);
        }
        for (i, p) in self.list.iter().enumerate().take(20) {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let r = R::new(area.x + 16.0 * s, y, (area.w - 32.0 * s).min(520.0 * s), 28.0 * s);
            g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, rgb(28, 32, 52));
            let fit = g.fit(13.0 * s, &name, Face::Sans, r.w - 16.0 * s);
            g.text(r.x + 10.0 * s, r.y + 6.0 * s, 13.0 * s, OK, &fit, Face::Sans);
            g.hit(r, Event::Row(i), &name);
            y += 32.0 * s;
        }
    }
}

#[allow(dead_code)]
fn _unused(_: BoxGlyphs) {}

#[cfg(test)]
#[path = "pdfview_tests.rs"]
mod tests;
