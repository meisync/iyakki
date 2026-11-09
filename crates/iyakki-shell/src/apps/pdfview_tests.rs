//! The PDF viewer through the tile harness: open, pages, zoom, search with a count, copy, errors, no overlapping controls.

use super::*;
use crate::apps::tests::cx_with_admin_store;
use crate::apps::tile_harness::TileHarness;
use iyakki_pdf::{Block, Builder, PageSize};

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
    let d = std::env::temp_dir().join(format!("iyakki-pdfview-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn harness(tag: &str, bytes: &[u8]) -> (TileHarness<PdfApp>, PathBuf) {
    let cx = cx_with_admin_store(tag);
    let d = dir(tag);
    let f = d.join("book.pdf");
    std::fs::write(&f, bytes).unwrap();
    let mut app = PdfApp::new(&cx);
    app.open_path(&f, &cx);
    (TileHarness::new(app, cx), d)
}

/// Heartbeats and drawing until `done` holds.
fn until(t: &mut TileHarness<PdfApp>, what: &str, done: impl Fn(&PdfView) -> bool) {
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
    let clip = crate::ui::clipboard::latest().unwrap_or_default();
    assert!(clip.contains("An elephant walks here."), "{clip}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn bad_files_say_what_is_wrong() {
    let cx = cx_with_admin_store("pv5");
    let d = dir("pv5");
    let mut app = PdfApp::new(&cx);
    let junk = d.join("junk.pdf");
    std::fs::write(&junk, b"this is not a pdf").unwrap();
    app.open_path(&junk, &cx);
    let mut t = TileHarness::new(app, cx.clone());
    until(&mut t, "error", |v| v.error.is_some());
    assert!(t.shows("this is not a PDF file"), "{:?}", t.texts());
    // password-protected
    let locked = d.join("locked.pdf");
    std::fs::write(&locked, b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\ntrailer\n<< /Root 1 0 R /Encrypt << /Filter /Standard >> >>\n%%EOF").unwrap();
    let mut app = PdfApp::new(&cx);
    app.open_path(&locked, &cx);
    let mut t = TileHarness::new(app, cx.clone());
    until(&mut t, "error", |v| v.error.is_some());
    assert!(t.shows("password-protected PDFs are not supported yet"));
    // a file outside the person's rules and a missing file
    let mut app = PdfApp::new(&cx);
    app.open_path(&d.join("nope.pdf"), &cx);
    assert!(app.view.error.is_some());
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
    let cx = cx_with_admin_store("pv8");
    let dl = super::super::browser::BrowserApp::downloads_dir(&cx).unwrap();
    std::fs::write(dl.join("listed.pdf"), sample(1)).unwrap();
    let app = PdfApp::new(&cx);
    let mut t = TileHarness::new(app, cx);
    assert!(t.shows("listed.pdf"), "{:?}", t.texts());
    t.click_label("listed.pdf");
    until(&mut t, "opened", |v| v.pages == 1);
    let _ = std::fs::remove_file(dl.join("listed.pdf"));
}
