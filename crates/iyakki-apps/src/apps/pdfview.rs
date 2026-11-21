//! The PDF viewer: the tile and the viewer component live in the `iyakki-tile-pdf` crate (it does not depend on the agent). This file is
//! the shell's side of it: the file rules, the Downloads folder and the audit line behind the tile's `PdfFiles`, the adapter that makes the
//! tile crate's `App` an `App` of the shell, and `PdfView` as the browser hosts it (its `handle` answers with the shell's `Flow`).
//! See docs/development/COMPILE-SPLIT-PILOT-5.md.

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_tile_api::pdf::PdfFiles;
use iyakki_tile_api::{App as TileApp, TileCx};

use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_pdf::{is_tab, ACCENT, TAB_BASE};

/// The real file rules, Downloads folder and audit log of the signed-in person.
struct ShellPdfFiles {
    downloads: Option<PathBuf>,
    policy: iyakki_sec::policy::Policy,
    user: String,
    audit: Arc<dyn iyakki_agent_api::AuditApi>,
}

impl PdfFiles for ShellPdfFiles {
    fn downloads_dir(&self) -> Option<PathBuf> {
        self.downloads.clone()
    }
    fn check_read(&self, path: &Path) -> Result<(), String> {
        crate::file_scope().check_read(&self.policy, &self.user, path)
    }
    fn opened(&self, file_name: &str) {
        self.audit.record(&self.user, "pdf.open", file_name);
    }
    fn check_write(&self, path: &Path) -> Result<(), String> {
        crate::file_scope().check_write(&self.policy, &self.user, path)
    }
    fn saved(&self, file_name: &str) {
        self.audit.record(&self.user, "pdf.form.save", file_name);
    }
}

fn files_of(cx: &AppCx) -> Arc<dyn PdfFiles> {
    Arc::new(ShellPdfFiles {
        downloads: super::browser::BrowserApp::downloads_dir(cx).ok(),
        policy: cx.session.policy.clone(),
        user: cx.session.user.clone(),
        audit: cx.audit.clone(),
    })
}

/// The viewer component as the browser hosts it: the tile crate's view, with `handle` answering in the shell's `Flow`. Everything else
/// (fields, `poll`, `render`, `shown_page` ...) is the tile crate's, through `Deref`.
pub struct PdfView(iyakki_tile_pdf::PdfView);

impl PdfView {
    pub fn open_file(path: &Path) -> PdfView {
        PdfView(iyakki_tile_pdf::PdfView::open_file(path))
    }
    /// The viewer as the browser hosts it, able to save a filled-in copy under the person's file rules.
    pub fn open_file_for(path: &Path, cx: &AppCx) -> PdfView {
        let mut v = iyakki_tile_pdf::PdfView::open_file(path);
        v.set_files(files_of(cx));
        PdfView(v)
    }
    pub fn hint() -> &'static str {
        iyakki_tile_pdf::PdfView::hint()
    }
    pub fn handle(&mut self, ev: &Event) -> Flow {
        match self.0.handle(ev) {
            iyakki_tile_api::Flow::Stay => Flow::Stay,
            iyakki_tile_api::Flow::Back => Flow::Back,
        }
    }
}

impl Deref for PdfView {
    type Target = iyakki_tile_pdf::PdfView;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for PdfView {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// The viewer's context menu (a long press on a touch screen, a right click) as the shell's menu entries.
pub fn menu_items(v: &iyakki_tile_pdf::PdfView) -> Vec<crate::ctxmenu::Item> {
    use crate::ctxmenu::Item;
    v.menu().into_iter().map(|(id, label, shortcut, on)| if id.is_empty() { Item::separator() } else { Item::new(id, label, shortcut).enabled(on) }).collect()
}

/// The tile crate's app, as an app of the shell.
pub struct PdfApp {
    inner: iyakki_tile_pdf::PdfApp,
}

impl PdfApp {
    pub fn new(cx: &AppCx) -> PdfApp {
        PdfApp { inner: iyakki_tile_pdf::PdfApp::new(files_of(cx)) }
    }

    /// Open a file after the file rules agree.
    pub fn open_path(&mut self, path: &Path, _cx: &AppCx) {
        self.inner.open_path(path);
    }

    pub fn view(&self) -> &iyakki_tile_pdf::PdfView {
        &self.inner.view
    }

    fn with<T>(cx: &AppCx, f: impl FnOnce(&TileCx) -> T) -> T {
        f(&TileCx {
            user: cx.session.user.as_str(),
            manage_settings: cx.session.policy.manage_settings,
            usage: &super::usage::ShellUsage::of(cx),
            calendar: &iyakki_tile_api::calendar::NoCalendar,
            audit: &iyakki_tile_api::NoAudit,
            remote: &iyakki_tile_api::remote::NoRemote,
            cameras: &iyakki_tile_api::cameras::NoCameras, radio: &iyakki_tile_api::radio::NoRadio })
    }
}

impl App for PdfApp {
    fn title(&self) -> &'static str {
        self.inner.title()
    }
    fn icon(&self) -> Icon {
        self.inner.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.inner.accent()
    }
    fn hint(&self) -> String {
        self.inner.hint()
    }
    fn help(&self) -> Vec<String> {
        self.inner.help()
    }
    fn open_item(&mut self, item: &str, _cx: &AppCx) {
        self.inner.open_item(item);
    }
    fn context_items(&mut self, _x: i32, _y: i32) -> Vec<crate::ctxmenu::Item> {
        if self.inner.view.is_open() { menu_items(&self.inner.view) } else { Vec::new() }
    }
    fn context_pick(&mut self, id: &str, _cx: &AppCx) -> Flow {
        self.inner.view.menu_pick(id);
        Flow::Stay
    }
    fn wants_redraw(&mut self) -> bool {
        self.inner.wants_redraw()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match Self::with(cx, |t| self.inner.handle(ev, t)) {
            iyakki_tile_api::Flow::Stay => Flow::Stay,
            iyakki_tile_api::Flow::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        Self::with(cx, |t| self.inner.render(g, area, s, t, tick));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use crate::apps::tile_harness::TileHarness;
    use crate::ui::Key;
    use iyakki_pdf::{Block, Builder, PageSize};

    fn sample() -> Vec<u8> {
        let mut b = Builder::new(PageSize::A4);
        b.title = "Zoo".into();
        b.blocks.push(Block::Heading(1, "Chapter 1".into()));
        b.blocks.push(Block::Para("An elephant walks here.".into()));
        b.build().0
    }

    /// The chat's request (`pdf_open`) is `file:PATH` and `page:N`: the viewer opens that file and shows that page, also when the pages are
    /// not known yet at the moment of the request.
    #[test]
    fn the_chats_request_opens_the_file_on_the_page_asked_for() {
        let cx = cx_with_admin_store("pdfopenpage");
        let dl = super::super::browser::BrowserApp::downloads_dir(&cx).unwrap();
        let mut b = Builder::new(PageSize::A4);
        for i in 0..400 {
            b.blocks.push(Block::Para(format!("Paragraph {i}: the elephant walks to the river and drinks for a long while before it goes home.")));
        }
        std::fs::write(dl.join("long.pdf"), b.build().0).unwrap();
        let mut app = PdfApp::new(&cx);
        app.open_item(&format!("file:{}\npage:3", dl.join("long.pdf").display()), &cx);
        let mut t = TileHarness::new(app, cx.clone());
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(36) {
            t.send(Event::Tick);
            t.draw();
            if t.app.view().pages >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        assert!(t.app.view().pages >= 3, "the file is long enough: {}", t.app.view().pages);
        assert_eq!(t.app.view().page, 2, "page 3 is shown (zero-based 2)");
        // a page past the end is the last page, and a request without a page leaves the first
        let mut app = PdfApp::new(&cx);
        app.open_item(&format!("file:{}\npage:9999", dl.join("long.pdf").display()), &cx);
        let mut t = TileHarness::new(app, cx.clone());
        for _ in 0..3000 {
            t.send(Event::Tick);
            t.draw();
            if t.app.view().pages > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        assert_eq!(t.app.view().page, t.app.view().pages - 1);
        let mut app = PdfApp::new(&cx);
        app.open_item(&format!("file:{}", dl.join("long.pdf").display()), &cx);
        assert_eq!(app.view().page, 0);
    }

    /// The shell's adapter wires the real parts: the Downloads folder lists, the file rules refuse, an opened file is audited, the viewer
    /// opens, and the browser's view answers in the shell's `Flow`.
    #[test]
    fn the_shell_adapter_feeds_the_real_rules_downloads_and_audit_to_the_tile_crate() {
        let cx = cx_with_admin_store("pdfshell");
        let dl = super::super::browser::BrowserApp::downloads_dir(&cx).unwrap();
        std::fs::write(dl.join("listed-by-shell.pdf"), sample()).unwrap();
        let app = PdfApp::new(&cx);
        let mut t = TileHarness::new(app, cx.clone());
        assert!(t.shows("listed-by-shell.pdf"), "{:?}", t.texts());
        t.click_label("listed-by-shell.pdf");
        for _ in 0..3000 {
            t.send(Event::Tick);
            t.draw();
            if t.app.view().pages == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        assert_eq!(t.app.view().pages, 1);
        assert!(matches!(t.key(Key::Esc), Flow::Back), "Esc closes the open document, then the tile");
        let mut v = PdfView::open_file(&dl.join("listed-by-shell.pdf"));
        assert!(matches!(v.handle(&Event::Key(Key::Esc)), Flow::Back), "the browser's component answers in the shell's Flow");
        assert_eq!(v.pages, 0, "Deref reaches the tile crate's fields (the worker has not answered yet)");
        let mut app = PdfApp::new(&cx);
        let nope = dl.join("not-there.pdf");
        app.open_path(&nope, &cx);
        assert!(app.view().error.is_some());
        let _ = std::fs::remove_file(dl.join("listed-by-shell.pdf"));
    }

    /// A long press or a right click on the viewer offers copy, select all and the page and zoom entries, and the shell carries the
    /// page ones out through the app.
    #[test]
    fn the_viewers_context_menu_has_copy_select_all_and_page_entries_and_they_work() {
        let cx = cx_with_admin_store("pdfmenu");
        let dl = super::super::browser::BrowserApp::downloads_dir(&cx).unwrap();
        std::fs::write(dl.join("menu-test.pdf"), sample()).unwrap();
        let mut app = PdfApp::new(&cx);
        assert!(app.context_items(10, 10).is_empty(), "nothing is open yet");
        app.open_path(&dl.join("menu-test.pdf"), &cx);
        for _ in 0..3000 {
            app.handle(&Event::Tick, &cx);
            if app.view().pages == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        let items = app.context_items(10, 10);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert!(ids.contains(&"copy") && ids.contains(&"selall") && ids.contains(&"pdf-zin") && ids.contains(&"pdf-fit"), "{ids:?}");
        assert!(items.iter().any(|i| i.is_separator()));
        let before = app.view().zoom;
        app.context_pick("pdf-zin", &cx);
        assert!(app.view().zoom > before || before >= 4.0);
        let _ = std::fs::remove_file(dl.join("menu-test.pdf"));
    }

    /// A form filled in the real viewer in the real shell: the copy goes next to the original under the person's own write rules, is
    /// audited, and never replaces the file.
    #[test]
    fn a_form_filled_in_the_shell_is_saved_as_a_copy_under_the_write_rules_and_audited() {
        let cx = cx_with_admin_store("pdfform");
        let dl = super::super::browser::BrowserApp::downloads_dir(&cx).unwrap();
        let objs: [(u32, &str); 4] = [
            (1, "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) >> >>"),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>"),
            (3, "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>"),
            (4, "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [10 70 110 90] >>"),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offs = vec![0usize; 5];
        for (n, b) in &objs {
            offs[*n as usize] = out.len();
            out.extend_from_slice(format!("{n} 0 obj\n{b}\nendobj\n").as_bytes());
        }
        let x = out.len();
        out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for n in 1..5 {
            out.extend_from_slice(format!("{:010} 00000 n \n", offs[n]).as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n").as_bytes());
        let _ = std::fs::remove_file(dl.join("shell-form (filled).pdf"));
        std::fs::write(dl.join("shell-form.pdf"), &out).unwrap();
        let mut app = PdfApp::new(&cx);
        app.open_path(&dl.join("shell-form.pdf"), &cx);
        let mut t = TileHarness::new(app, cx.clone());
        for _ in 0..3000 {
            t.send(Event::Tick);
            t.draw();
            if !t.app.view().fields.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        t.click_label("Form (1)");
        t.click_label("Fill in name");
        for _ in 0..3 {
            t.key(Key::Backspace);
        }
        for c in "Grace".chars() {
            t.key(Key::Char(c));
        }
        t.key(Key::Enter);
        t.click_label("Save copy (1)");
        for _ in 0..3000 {
            t.send(Event::Tick);
            t.draw();
            if t.app.view().status.as_deref().is_some_and(|s| s.starts_with("Saved") || s.starts_with("Not saved")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        let status = t.app.view().status.clone().unwrap_or_default();
        assert!(status.starts_with("Saved shell-form (filled).pdf"), "{status}");
        assert_eq!(std::fs::read(dl.join("shell-form.pdf")).unwrap(), out, "the original is untouched");
        let d = iyakki_pdf::Document::open(std::fs::read(dl.join("shell-form (filled).pdf")).unwrap()).unwrap();
        assert_eq!(d.editable_fields()[0].value, "Grace");
        let log = std::fs::read_to_string(cx.audit.path_buf()).unwrap_or_default();
        assert!(log.contains("pdf.form.save") && log.contains("shell-form (filled).pdf"), "{log}");
        let _ = std::fs::remove_file(dl.join("shell-form.pdf"));
        let _ = std::fs::remove_file(dl.join("shell-form (filled).pdf"));
    }
}
