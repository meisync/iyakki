//! The PDF viewer through a small headless harness and a fake `PdfFiles`: open, pages, zoom, search with a count, copy, errors, no
//! overlapping controls. (Moved from the shell's `pdfview_tests.rs`; the tests need the toolkit and the reader library, not the agent.)

use super::*;
use iyakki_pdf::{Block, Builder, PageSize};
use std::path::Path;
use std::sync::Mutex;

fn sample(pages: usize) -> Vec<u8> {
    let mut b = Builder::new(PageSize::A4);
    b.title = "Zoo".into();
    for p in 0..pages {
        if p > 0 {
            // push to a new page with filler
            for _ in 0..70 {
                b.blocks.push(Block::Para("filler line that takes room on the page, again and again.".into()));
            }
        }
        b.blocks.push(Block::Heading(1, format!("Chapter {}", p + 1)));
        b.blocks.push(Block::Para(format!("An elephant walks here. Another elephant follows on page marker {}.", p + 1)));
    }
    b.build().0
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-tile-pdf-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The shell's side, faked: a Downloads folder, a file rule that refuses what is under `denied`, and the audit lines it was told about.
struct Files {
    downloads: Option<PathBuf>,
    denied: Option<PathBuf>,
    /// Where a copy may not be written.
    denied_write: Option<PathBuf>,
    audit: Mutex<Vec<String>>,
}

impl Files {
    fn new(downloads: Option<PathBuf>) -> Arc<Files> {
        Arc::new(Files { downloads, denied: None, denied_write: None, audit: Mutex::new(Vec::new()) })
    }
}

impl PdfFiles for Files {
    fn downloads_dir(&self) -> Option<PathBuf> {
        self.downloads.clone()
    }
    fn check_read(&self, path: &Path) -> Result<(), String> {
        match &self.denied {
            Some(d) if path.starts_with(d) => Err(format!("{} is outside the files this person may read", path.display())),
            _ => Ok(()),
        }
    }
    fn opened(&self, file_name: &str) {
        self.audit.lock().unwrap().push(file_name.to_string());
    }
    fn check_write(&self, path: &Path) -> Result<(), String> {
        match &self.denied_write {
            Some(d) if path.starts_with(d) => Err(format!("{} is outside the files this person may change", path.display())),
            _ => Ok(()),
        }
    }
    fn saved(&self, file_name: &str) {
        self.audit.lock().unwrap().push(format!("saved:{file_name}"));
    }
}

struct NoUse;
impl iyakki_tile_api::usage::UsageSource for NoUse {
    fn now_unix(&self) -> i64 {
        0
    }
    fn series(&self, _: iyakki_tile_api::usage::Range, _: i64, _: Option<&str>) -> iyakki_tile_api::usage::Series {
        iyakki_tile_api::usage::Series { models: vec![], buckets: vec![], totals: vec![], input: 0, output: 0, calls: 0 }
    }
}

/// The four window sizes every tile must fit, and the shell's header above the tile.
const STANDARD_SIZES: [(u32, u32); 4] = [(1280, 720), (1024, 600), (800, 480), (480, 600)];
const HEADER: f32 = 72.0;

/// A small headless harness (the shell's `TileHarness` needs the agent; this one needs only the toolkit).
struct H {
    app: PdfApp,
    size: (u32, u32),
    last: Option<Gfx>,
}

fn with_cx<T>(f: impl FnOnce(&TileCx) -> T) -> T {
    f(&TileCx { user: "amy", manage_settings: false, usage: &NoUse, calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: &iyakki_tile_api::remote::NoRemote, cameras: &iyakki_tile_api::cameras::NoCameras, radio: &iyakki_tile_api::radio::NoRadio })
}

fn partly_overlap(a: R, b: R) -> bool {
    let iw = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let ih = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if iw <= 1.0 || ih <= 1.0 {
        return false;
    }
    let inside = |i: R, o: R| i.x >= o.x - 1.0 && i.y >= o.y - 1.0 && i.x + i.w <= o.x + o.w + 1.0 && i.y + i.h <= o.y + o.h + 1.0;
    !(inside(a, b) || inside(b, a))
}

impl H {
    fn new(app: PdfApp) -> H {
        H { app, size: STANDARD_SIZES[0], last: None }
    }
    fn send(&mut self, ev: Event) -> Flow {
        self.last = None;
        with_cx(|cx| self.app.handle(&ev, cx))
    }
    fn key(&mut self, k: Key) -> Flow {
        self.send(Event::Key(k))
    }
    fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            self.key(Key::Char(c));
        }
    }
    fn draw(&mut self) -> &Gfx {
        if self.last.is_none() {
            let (w, h) = self.size;
            let mut g = Gfx::new(w, h);
            g.clear(rgb(9, 11, 20));
            let s = (w as f32 / 1280.0).clamp(0.6, 3.0);
            with_cx(|cx| self.app.render(&mut g, R::new(0.0, HEADER, w as f32, h as f32 - HEADER), s, cx, 0));
            self.last = Some(g);
        }
        self.last.as_ref().unwrap()
    }
    fn draw_at(&mut self, w: u32, h: u32) -> &Gfx {
        if self.size != (w, h) {
            self.size = (w, h);
            self.last = None;
        }
        self.draw()
    }
    fn has_button(&mut self, label: &str) -> bool {
        self.draw().hits.iter().any(|h| h.label == label)
    }
    fn click_label(&mut self, label: &str) -> Flow {
        let hits = self.draw().hits.clone();
        let Some(hit) = hits.iter().rev().find(|h| h.label == label) else {
            let names: Vec<&str> = hits.iter().map(|h| h.label.as_str()).filter(|l| !l.is_empty()).collect();
            panic!("no control called {label:?} at {:?}; there is {names:?}", self.size);
        };
        self.send(hit.ev.clone())
    }
    fn shows(&mut self, needle: &str) -> bool {
        self.draw().text_log.iter().any(|t| t.2.contains(needle))
    }
    fn texts(&mut self) -> Vec<String> {
        self.draw().text_log.iter().map(|t| t.2.clone()).collect()
    }
    fn assert_no_overlap_at_standard_sizes(&mut self) {
        for (w, h) in STANDARD_SIZES {
            let hits = self.draw_at(w, h).hits.clone();
            for (i, a) in hits.iter().enumerate() {
                for b in &hits[i + 1..] {
                    assert!(!partly_overlap(a.rect, b.rect), "{w}x{h}: {:?} {:?} overlaps {:?} {:?}", a.label, a.rect, b.label, b.rect);
                }
            }
        }
        self.draw_at(STANDARD_SIZES[0].0, STANDARD_SIZES[0].1);
    }
    fn draw_at_standard_sizes(&mut self) {
        for (w, h) in STANDARD_SIZES {
            self.draw_at(w, h);
        }
        self.draw_at(STANDARD_SIZES[0].0, STANDARD_SIZES[0].1);
    }
}

fn harness(tag: &str, bytes: &[u8]) -> (H, PathBuf) {
    let d = dir(tag);
    let f = d.join("book.pdf");
    std::fs::write(&f, bytes).unwrap();
    let mut app = PdfApp::new(Files::new(Some(d.clone())));
    app.open_path(&f);
    (H::new(app), d)
}

/// Heartbeats and drawing until `done` holds.
fn until(t: &mut H, what: &str, done: impl Fn(&PdfView) -> bool) {
    for _ in 0..3000 {
        t.send(Event::Tick);
        t.draw();
        if done(&t.app.view) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    panic!("never happened: {what}; status {:?} error {:?}", t.app.view.status, t.app.view.error);
}

#[test]
fn a_pdf_opens_shows_its_pages_and_moves_between_them() {
    let (mut t, d) = harness("pv1", &sample(3));
    until(&mut t, "opened", |v| v.pages > 0);
    let n = t.app.view.pages;
    assert!(n >= 3, "three chapters with filler make at least three pages: {n}");
    assert!(t.shows(&format!("Page 1 / {n}")), "{:?}", t.texts());
    until(&mut t, "page drawn", |v| v.shown_page() == Some(0));
    let (w, h, px) = t.app.view.shown_pixels().unwrap();
    assert!(w > 300 && h > 400);
    assert!(px.chunks(4).any(|p| p[0] < 80 && p[1] < 80 && p[2] < 80), "dark text pixels from the shell's own font");
    t.click_label("Next");
    until(&mut t, "page 2", |v| v.page == 1 && v.shown_page() == Some(1));
    assert!(t.shows(&format!("Page 2 / {n}")));
    for _ in 0..n + 2 {
        t.key(Key::Char('n'));
    }
    assert_eq!(t.app.view.page, n - 1, "the last page stays");
    t.key(Key::Home);
    assert_eq!(t.app.view.page, 0);
    t.key(Key::End);
    assert_eq!(t.app.view.page, n - 1);
    t.click_label("Page 1");
    assert_eq!(t.app.view.page, 0, "the page list jumps");
    assert!(t.shows("Zoo") || t.app.view.title == "Zoo");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn zoom_and_fit_width_change_the_picture_size() {
    let (mut t, d) = harness("pv2", &sample(1));
    until(&mut t, "page drawn", |v| v.shown_page() == Some(0));
    let (w0, _, _) = t.app.view.shown_pixels().unwrap();
    assert!(!t.has_button("Fit width") || t.app.view.fit_width == false);
    t.click_label("+");
    assert!(!t.app.view.fit_width);
    let z = t.app.view.zoom;
    until(&mut t, "re-rendered", |v| v.shown_pixels().is_some_and(|(w, _, _)| w != w0));
    t.click_label("-");
    t.click_label("-");
    assert!(t.app.view.zoom < z);
    t.click_label("Fit width");
    assert!(t.app.view.fit_width);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn search_counts_matches_across_pages_and_steps_through_them() {
    let (mut t, d) = harness("pv3", &sample(3));
    until(&mut t, "opened", |v| v.pages >= 3);
    t.key(Key::Char('/'));
    assert!(t.app.view.search_open);
    t.type_text("elephant");
    t.key(Key::Enter);
    until(&mut t, "search finished", |v| !v.searching && v.query == "elephant");
    // two "elephant" per chapter paragraph on each of 3 pages
    assert_eq!(t.app.view.hits.len(), 6);
    assert!(t.shows("1 of 6"), "{:?}", t.texts());
    assert!(t.shows("Page 1 · 2 matches"), "{:?}", t.texts());
    t.click_label("Next match");
    assert_eq!(t.app.view.hit, 1);
    t.click_label("Next match");
    assert!(t.app.view.page >= 1, "the third match is on a later page");
    until(&mut t, "that page drawn", |v| v.shown_page() == Some(v.page));
    // highlights are drawn over the page: yellow or orange pixels appear
    let g = t.draw();
    let rgba = g.rgba();
    assert!(rgba.chunks(4).any(|p| p[0] > 200 && p[1] > 120 && p[2] < 120), "a highlight colour is on screen");
    t.key(Key::Esc);
    t.key(Key::Esc);
    t.type_text("");
    // a query with no match
    t.key(Key::Char('/'));
    t.key(Key::Ctrl('a'));
    t.type_text("zebra");
    t.key(Key::Enter);
    until(&mut t, "no matches", |v| !v.searching && v.query == "zebra");
    assert!(t.shows("No matches"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn copy_text_puts_the_page_text_on_the_clipboard_and_says_so() {
    let (mut t, d) = harness("pv4", &sample(1));
    until(&mut t, "opened", |v| v.pages == 1);
    t.key(Key::Char('c'));
    until(&mut t, "copied", |v| v.status.as_deref().is_some_and(|s| s.starts_with("Copied the text of page 1")));
    let clip = iyakki_uikit::ui::clipboard::latest().unwrap_or_default();
    assert!(clip.contains("An elephant walks here."), "{clip}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn bad_files_say_what_is_wrong() {
    let d = dir("pv5");
    let files = Arc::new(Files { downloads: None, denied: Some(d.join("private")), denied_write: None, audit: Mutex::new(Vec::new()) });
    let mut app = PdfApp::new(files.clone());
    let junk = d.join("junk.pdf");
    std::fs::write(&junk, b"this is not a pdf").unwrap();
    app.open_path(&junk);
    let mut t = H::new(app);
    until(&mut t, "error", |v| v.error.is_some());
    assert!(t.shows("this is not a PDF file"), "{:?}", t.texts());
    // password-protected
    let locked = d.join("locked.pdf");
    std::fs::write(&locked, b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\ntrailer\n<< /Root 1 0 R /Encrypt << /Filter /Standard >> >>\n%%EOF").unwrap();
    let mut app = PdfApp::new(files.clone());
    app.open_path(&locked);
    let mut t = H::new(app);
    until(&mut t, "error", |v| v.error.is_some());
    assert!(t.shows("protection data of this PDF is damaged"));
    // a file outside the person's rules and a missing file
    let mut app = PdfApp::new(files.clone());
    app.open_path(&d.join("nope.pdf"));
    assert!(app.view.error.is_some());
    // the file rules of the person say no: the viewer shows their sentence and no audit line is written
    std::fs::create_dir_all(d.join("private")).unwrap();
    std::fs::write(d.join("private/secret.pdf"), sample(1)).unwrap();
    let before = files.audit.lock().unwrap().len();
    app.open_path(&d.join("private/secret.pdf"));
    assert!(app.view.error.as_deref().is_some_and(|e| e.contains("outside the files")), "{:?}", app.view.error);
    assert_eq!(app.view.pages, 0);
    assert_eq!(files.audit.lock().unwrap().len(), before, "a refused file is not audited as opened");
    assert!(files.audit.lock().unwrap().iter().all(|n| n != "secret.pdf"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn what_cannot_be_shown_is_listed_under_the_page() {
    // a page with a gradient (shading) and a JPEG 2000 picture
    use iyakki_pdf::Builder as _B;
    let _ = _B::new(PageSize::A4);
    let content = "/Sh1 sh BT /F1 12 Tf 20 50 Td (Hello) Tj ET";
    let pdf = format!(
        "%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n4 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\n5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF",
        content.len()
    );
    let (mut t, d) = harness("pv6", pdf.as_bytes());
    until(&mut t, "page drawn", |v| v.shown_page() == Some(0));
    t.send(Event::Tick);
    assert!(t.texts().iter().any(|x| x.contains("not shown on this page") && x.contains("shading")), "{:?}", t.texts());
    assert!(t.app.view.repaired, "no cross-reference table: the file was rebuilt");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_controls_do_not_overlap_at_the_standard_sizes() {
    let (mut t, d) = harness("pv7", &sample(2));
    until(&mut t, "page drawn", |v| v.shown_page() == Some(0));
    t.key(Key::Char('/'));
    t.assert_no_overlap_at_standard_sizes();
    t.draw_at_standard_sizes();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_empty_tile_lists_downloaded_pdfs() {
    let dl = dir("pv8");
    std::fs::write(dl.join("listed.pdf"), sample(1)).unwrap();
    let files = Files::new(Some(dl.clone()));
    let app = PdfApp::new(files.clone());
    let mut t = H::new(app);
    assert!(t.shows("listed.pdf"), "{:?}", t.texts());
    t.click_label("listed.pdf");
    until(&mut t, "opened", |v| v.pages == 1);
    assert_eq!(files.audit.lock().unwrap().as_slice(), ["listed.pdf".to_string()], "opening is audited by file name");
    let _ = std::fs::remove_dir_all(dl);
}

// ---- selecting text, copy, the keys touch gestures turn into, the context menu ----------------------------------------------------

/// Where (in the page area's pixels) the middle of character `i` of the shown page is.
fn char_px(v: &PdfView, i: usize) -> (f32, f32) {
    let chars = v.page_chars().expect("the characters arrived");
    let m = v.shown.as_ref().unwrap().matrix;
    let r = chars[i].rect;
    let (ux, uy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    ((m[0] * ux + m[2] * uy + m[4]) as f32 + v.page_org.0, (m[1] * ux + m[3] * uy + m[5]) as f32 + v.page_org.1)
}

fn press(t: &mut H, at: (f32, f32), clicks: u8, dragging: bool) {
    iyakki_uikit::ui::set_click_count(clicks);
    iyakki_uikit::ui::set_dragging(dragging);
    t.send(Event::AreaAt(AREA_PAGE, at.0, at.1));
    iyakki_uikit::ui::set_dragging(false);
    iyakki_uikit::ui::set_click_count(1);
}

fn open_with_chars(tag: &str) -> (H, PathBuf) {
    let (mut t, d) = harness(tag, &sample(1));
    until(&mut t, "page and characters", |v| v.shown_page() == Some(0) && v.page_chars().is_some());
    t.draw();
    (t, d)
}

fn index_of(v: &PdfView, text: &str) -> usize {
    let chars = v.page_chars().unwrap();
    let all: String = chars.iter().map(|c| c.ch).collect();
    all.find(text).map(|b| all[..b].chars().count()).unwrap_or_else(|| panic!("{text:?} not in {all:?}"))
}

#[test]
fn dragging_across_the_page_selects_text_draws_it_and_copy_takes_it() {
    let (mut t, d) = open_with_chars("pvsel1");
    let start = index_of(&t.app.view, "elephant walks");
    let (a, b) = (char_px(&t.app.view, start), char_px(&t.app.view, start + "elephant walks".len() - 1));
    press(&mut t, a, 1, false);
    assert!(t.app.view.sel.is_none(), "a plain press selects nothing yet");
    press(&mut t, (a.0 + (b.0 - a.0) / 2.0, b.1), 1, true);
    press(&mut t, b, 1, true);
    assert_eq!(t.app.view.selected_text().as_deref(), Some("elephant walks"));
    // it is drawn: blue boxes over the page that were not there
    let before = { let g = t.draw(); g.rgba().chunks(4).filter(|p| p[0] > 150 && p[0] < 200 && p[2] > 240).count() };
    assert!(before > 50, "the selection is drawn ({before})");
    t.key(Key::Char('c'));
    assert_eq!(iyakki_uikit::ui::clipboard::latest().as_deref(), Some("elephant walks"));
    assert!(t.app.view.status.as_deref().unwrap().contains("Copied the selected text (14 characters)"), "{:?}", t.app.view.status);
    // Esc clears the selection first, and only then leaves
    assert!(matches!(t.key(Key::Esc), Flow::Stay));
    assert!(t.app.view.sel.is_none());
    let after = { let g = t.draw(); g.rgba().chunks(4).filter(|p| p[0] > 150 && p[0] < 200 && p[2] > 240).count() };
    assert!(after < before / 2, "and the boxes are gone ({after})");
    // with nothing selected, c copies the whole page as before
    t.key(Key::Char('c'));
    until(&mut t, "page copied", |v| v.status.as_deref().is_some_and(|s| s.starts_with("Copied the text of page 1")));
    // dragging backwards and across lines also works
    let (first, last) = (index_of(&t.app.view, "Chapter"), index_of(&t.app.view, "walks"));
    let (p1, p2) = (char_px(&t.app.view, last + 2), char_px(&t.app.view, first));
    press(&mut t, p1, 1, false);
    press(&mut t, p2, 1, true);
    let text = t.app.view.selected_text().unwrap();
    assert!(text.starts_with("Chapter 1\nAn elephant wa"), "{text:?}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_double_click_takes_a_word_a_triple_click_a_line_and_ctrl_a_the_page() {
    let (mut t, d) = open_with_chars("pvsel2");
    let w = index_of(&t.app.view, "Another");
    let at = char_px(&t.app.view, w + 2);
    press(&mut t, at, 2, false);
    assert_eq!(t.app.view.selected_text().as_deref(), Some("Another"));
    press(&mut t, at, 3, false);
    let line = t.app.view.selected_text().unwrap();
    assert!(line.starts_with("An elephant walks here.") && line.contains("Another elephant"), "{line:?}");
    t.key(Key::Ctrl('a'));
    let all = t.app.view.selected_text().unwrap();
    assert!(all.contains("Chapter 1") && all.contains("page marker 1"), "{all:?}");
    // a plain click away from the text clears it
    press(&mut t, (5.0, 5.0), 1, false);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_press_before_the_characters_arrive_is_kept_and_a_new_page_drops_the_selection() {
    let (mut t, d) = harness("pvsel3", &sample(3));
    until(&mut t, "page drawn", |v| v.shown_page() == Some(0));
    t.key(Key::Ctrl('a'));
    t.key(Key::Char('n'));
    assert!(t.app.view.sel.is_none(), "the selection belongs to its page");
    until(&mut t, "page 2 and its characters", |v| v.page == 1 && v.shown_page() == Some(1) && v.page_chars().is_some());
    t.draw();
    t.key(Key::Ctrl('a'));
    assert!(t.app.view.selected_text().is_some());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_keys_a_pinch_and_a_two_finger_swipe_become_zoom_and_page_turns_and_the_menu_offers_the_same() {
    let (mut t, d) = harness("pvsel4", &sample(3));
    until(&mut t, "opened", |v| v.pages >= 3 && v.shown_page() == Some(0));
    t.key(Key::Char('-'));
    let z0 = t.app.view.zoom;
    t.key(Key::Ctrl('+'));
    assert!(t.app.view.zoom > z0, "pinching out zooms in");
    let z1 = t.app.view.zoom;
    t.key(Key::Ctrl('-'));
    assert!(t.app.view.zoom < z1, "pinching in zooms out");
    let right = Key::Nav { nav: iyakki_uikit::ui::Nav::Right, shift: false, ctrl: true };
    let left = Key::Nav { nav: iyakki_uikit::ui::Nav::Left, shift: false, ctrl: true };
    t.key(right.clone());
    assert_eq!(t.app.view.page, 1, "a swipe to the left turns to the next page");
    t.key(left.clone());
    assert_eq!(t.app.view.page, 0);
    t.key(left);
    assert_eq!(t.app.view.page, 0, "and not before the first");
    // the context menu (a long press on a touch screen)
    let m = t.app.view.menu();
    assert!(m.iter().any(|e| e.0 == "copy") && m.iter().any(|e| e.0 == "selall"));
    assert!(m.iter().find(|e| e.0 == "pdf-prev").is_some_and(|e| !e.3), "no previous page on the first");
    t.app.view.menu_pick("pdf-next");
    assert_eq!(t.app.view.page, 1);
    t.app.view.menu_pick("pdf-zin");
    t.app.view.menu_pick("pdf-fit");
    assert!(t.app.view.fit_width);
    let _ = std::fs::remove_dir_all(d);
}

const PROTECTED: &[u8] = include_bytes!("../../iyakki-pdf/tests/data/aes-128.pdf");

#[test]
fn a_protected_pdf_asks_for_its_password_refuses_a_wrong_one_and_opens_with_the_right_one_and_the_password_is_not_kept() {
    let (mut t, d) = harness("pv-pw", PROTECTED);
    until(&mut t, "the question", |v| v.password.is_some());
    assert!(t.shows("is protected with a password"), "{:?}", t.texts());
    assert!(!t.shows("open-sesame"));
    // the field is masked: what is typed is never drawn
    t.type_text("nope");
    assert!(!t.texts().iter().any(|x| x.contains("nope")), "{:?}", t.texts());
    t.key(Key::Enter);
    until(&mut t, "the second question", |v| matches!(&v.password, Some((_, true))));
    assert!(t.shows("That password does not open it"), "{:?}", t.texts());
    assert!(t.app.view.password.as_ref().is_some_and(|(f, _)| f.value.is_empty()), "the wrong password was taken out of the field");
    // a pasted password works like a typed one
    t.send(Event::Paste("open-sesame".into()));
    t.key(Key::Enter);
    until(&mut t, "opened", |v| v.pages == 1);
    assert!(t.app.view.password.is_none());
    until(&mut t, "page drawn", |v| v.shown_page() == Some(0));
    let (_, _, px) = t.app.view.shown_pixels().unwrap();
    assert!(px.chunks(4).any(|p| p[0] < 120 && p[1] < 120 && p[2] < 120), "the decrypted page has ink");
    assert!(!t.shows("protected with a password"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn escape_closes_the_password_question_and_other_files_are_not_asked() {
    let (mut t, d) = harness("pv-pw2", PROTECTED);
    until(&mut t, "the question", |v| v.password.is_some());
    assert!(matches!(t.key(Key::Esc), Flow::Back));
    let (mut t2, d2) = harness("pv-pw3", &sample(1));
    until(&mut t2, "opened", |v| v.pages > 0);
    assert!(t2.app.view.password.is_none(), "an ordinary file opens without a question");
    let _ = std::fs::remove_dir_all(d);
    let _ = std::fs::remove_dir_all(d2);
}

/// A one-page form: a text field, a read-only field, a check box and a choice.
fn form_bytes() -> Vec<u8> {
    let objs: [(u32, &str); 7] = [
        (1, "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R 7 0 R] /DA (/Helv 0 Tf 0 g) >> >>"),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>"),
        (3, "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 7 0 R] >>"),
        (4, "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [10 70 110 90] >>"),
        (5, "<< /Type /Annot /Subtype /Widget /FT /Tx /Ff 1 /T (locked) /V (fixed) /Rect [120 70 190 90] >>"),
        (6, "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [10 40 25 55] /AP << /N << /Yes 3 0 R /Off 3 0 R >> >> >>"),
        (7, "<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131072 /T (colour) /V (red) /Opt [(red) (green) (blue)] /Rect [40 40 110 55] >>"),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offs = vec![0usize; 8];
    for (n, b) in &objs {
        offs[*n as usize] = out.len();
        out.extend_from_slice(format!("{n} 0 obj\n{b}\nendobj\n").as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(b"xref\n0 8\n0000000000 65535 f \n");
    for n in 1..8 {
        out.extend_from_slice(format!("{:010} 00000 n \n", offs[n]).as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n").as_bytes());
    out
}

fn form_harness(tag: &str, files: Arc<Files>) -> (H, PathBuf) {
    let d = dir(tag);
    let f = d.join("tax.pdf");
    std::fs::write(&f, form_bytes()).unwrap();
    let mut app = PdfApp::new(files);
    app.open_path(&f);
    (H::new(app), d)
}

#[test]
fn a_form_is_filled_in_the_viewer_and_saved_as_a_new_file_next_to_the_original() {
    let d0 = dir("pvform1");
    let files = Files::new(Some(d0.clone()));
    let _ = std::fs::remove_dir_all(&d0);
    let (mut t, d) = form_harness("pvform1", files.clone());
    until(&mut t, "opened with fields", |v| v.pages > 0 && !v.fields.is_empty());
    assert_eq!(t.app.view.fillable().len(), 3, "the read-only field is not offered");
    t.click_label("Form (4)");
    assert!(t.shows("Form fields (click to fill)"), "{:?}", t.texts());
    assert!(!t.has_button("Fill in locked"), "a read-only field has no way in");
    // a text field: the bar opens with the old value; Backspace clears; Enter keeps it
    t.click_label("Fill in name");
    assert!(t.shows("name:") && t.shows("Enter keeps it"), "{:?}", t.texts());
    t.assert_no_overlap_at_standard_sizes();
    for _ in 0..3 {
        t.key(Key::Backspace);
    }
    t.type_text("Grace");
    t.key(Key::Enter);
    assert_eq!(t.app.view.pending(), &[("name".to_string(), "Grace".to_string())]);
    assert!(t.shows("* name (text): Grace"), "{:?}", t.texts());
    // Esc leaves the old value alone
    t.click_label("Fill in name");
    t.type_text("zzz");
    t.key(Key::Esc);
    assert_eq!(t.app.view.pending()[0].1, "Grace");
    // a check box flips, twice drops the change; a choice goes to the next option
    t.click_label("Fill in agree");
    assert_eq!(t.app.view.pending().len(), 2);
    t.click_label("Fill in agree");
    assert_eq!(t.app.view.pending().len(), 1, "back to the original: nothing to save for it");
    t.click_label("Fill in agree");
    t.click_label("Fill in colour");
    assert!(t.app.view.pending().contains(&("colour".to_string(), "green".to_string())));
    assert!(t.has_button("Save copy (3)"));
    t.assert_no_overlap_at_standard_sizes();
    let before = std::fs::read(d.join("tax.pdf")).unwrap();
    t.click_label("Save copy (3)");
    until(&mut t, "saved", |v| v.status.as_deref().is_some_and(|s| s.starts_with("Saved")));
    let msg = t.app.view.status.clone().unwrap();
    assert!(msg.contains("tax (filled).pdf") && msg.contains("3 fields") && msg.contains("original is unchanged"), "{msg}");
    assert_eq!(std::fs::read(d.join("tax.pdf")).unwrap(), before, "the original is untouched");
    let new = iyakki_pdf::Document::open(std::fs::read(d.join("tax (filled).pdf")).unwrap()).unwrap();
    let v = |n: &str| new.editable_fields().into_iter().find(|f| f.name == n).unwrap().value;
    assert_eq!((v("name").as_str(), v("agree").as_str(), v("colour").as_str(), v("locked").as_str()), ("Grace", "Yes", "green", "fixed"));
    assert!(files.audit.lock().unwrap().iter().any(|a| a == "saved:tax (filled).pdf"));
    assert!(t.app.view.pending().is_empty() && !t.has_button("Save copy (3)"));
    // another change and save makes a second copy: nothing is overwritten
    t.click_label("Fill in colour");
    t.click_label("Save copy (1)");
    until(&mut t, "saved again", |v| v.status.as_deref().is_some_and(|s| s.starts_with("Saved")));
    assert!(d.join("tax (filled 2).pdf").is_file() && d.join("tax (filled).pdf").is_file());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn saving_follows_the_file_rules_and_a_file_that_cannot_be_filled_says_so() {
    let d0 = dir("pvform2");
    let _ = std::fs::remove_dir_all(&d0);
    let files = Arc::new(Files { downloads: Some(d0.clone()), denied: None, denied_write: Some(d0.clone()), audit: Mutex::new(Vec::new()) });
    let (mut t, d) = form_harness("pvform2", files);
    until(&mut t, "opened", |v| v.pages > 0 && !v.fields.is_empty());
    t.click_label("Form (4)");
    t.click_label("Fill in agree");
    t.click_label("Save copy (1)");
    until(&mut t, "refused", |v| v.status.as_deref().is_some_and(|s| s.starts_with("Not saved")));
    assert!(t.app.view.status.clone().unwrap().contains("may not change") || t.app.view.status.clone().unwrap().contains("may change"), "{:?}", t.app.view.status);
    assert!(!d.join("tax (filled).pdf").exists(), "nothing was written");
    assert_eq!(t.app.view.pending().len(), 1, "the change is kept so the person can try again");
    // a file without a form has nothing to fill in
    let (mut p, d2) = harness("pvform3", &sample(1));
    until(&mut p, "page drawn", |v| v.shown_page() == Some(0));
    assert!(p.app.view.fillable().is_empty() && !p.has_button("Form (0)"));
    let _ = std::fs::remove_dir_all(d);
    let _ = std::fs::remove_dir_all(d2);
}
