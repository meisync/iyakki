//! The browser's document handling for PDFs (docs/PDF.md): a PDF file page opens in the viewer (key O, after the file is saved to
//! Downloads), a saved PDF opens from the Downloads list, and Ctrl+P saves the page as a PDF (the reader version: headings, text,
//! lists, tables as text, the pictures that have arrived). Ctrl+Shift+P switches the page size between A4 and Letter.
//!
//! The full web engine (WPE WebKit) is not asked to print: its API has no print-to-PDF, so a page shown by it is saved from the
//! built-in reader version instead (the status line says so).

use super::*;
use iyakki_pdf::PageSize;

impl BrowserApp {
    pub(super) fn pdf_size(&self) -> PageSize {
        PageSize::from_name(&self.data.pdf_size).unwrap_or(PageSize::A4)
    }

    /// Show a saved PDF in the viewer, if the person's file rules let them read it.
    pub(super) fn open_pdf_file(&mut self, path: &std::path::Path, cx: &AppCx) {
        if let Err(e) = iyakki_agent::scope::check_read(&cx.session.policy, &cx.session.user, path) {
            self.status = Some((e, false));
            return;
        }
        let v = super::super::pdfview::PdfView::open_file(path);
        cx.audit.record(&cx.session.user, "pdf.open", &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        self.pdf = Some(v);
        self.addr_focus = false;
    }

    /// Key O on a PDF file page: save it, then open it.
    pub(super) fn open_pdf_page(&mut self, cx: &AppCx) {
        let Some(page) = self.page().cloned().filter(|p| p.download && p.ctype.to_ascii_lowercase().contains("pdf")) else {
            self.status = Some(("This is not a PDF file.".into(), false));
            return;
        };
        self.pdf_after = Some(String::new());
        self.start_download_of(page.url.clone(), page.ctype.clone(), None, cx);
        if self.download.is_none() {
            self.pdf_after = None; // refused to start; the status says why
        } else {
            self.status = Some(("Saving the PDF to Downloads, then opening it…".into(), true));
        }
    }

    /// A download finished: open it when it was asked for as a PDF to read.
    pub(super) fn pdf_download_done(&mut self, name: &str, cx: &AppCx) {
        if self.pdf_after.take().is_none() {
            return;
        }
        match Self::downloads_dir(cx) {
            Ok(d) => self.open_pdf_file(&d.join(name), cx),
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// Ctrl+P: the page as a PDF in Downloads.
    pub(super) fn save_page_pdf(&mut self, cx: &AppCx) {
        if self.full.is_some() {
            self.status = Some(("The full engine cannot print to PDF (its WPE port has no print support). Press Esc for the built-in reader, then Ctrl+P saves its version of the page.".into(), false));
            return;
        }
        let Some(page) = self.page().cloned() else {
            self.status = Some(("Open a page first.".into(), false));
            return;
        };
        if page.download || page.media {
            self.status = Some(("This is a file, not a page: D saves it as it is.".into(), false));
            return;
        }
        let images: Vec<ImgState> = self.history.get(self.pos).map(|e| e.images.clone()).unwrap_or_default();
        let get = |i: usize| -> Option<iyakki_pdf::PdfImage> {
            match images.get(i)? {
                ImgState::Ready(pm) => {
                    let mut rgba = pm.data().to_vec();
                    for p in rgba.chunks_exact_mut(4) {
                        let a = p[3] as u32;
                        if a > 0 && a < 255 {
                            for c in p.iter_mut().take(3) {
                                *c = ((*c as u32 * 255 / a).min(255)) as u8;
                            }
                        }
                    }
                    iyakki_pdf::PdfImage::from_rgba(pm.width(), pm.height(), &rgba)
                }
                _ => None,
            }
        };
        let blocks = iyakki_pdf::docconv::blocks_from_doc(&page.doc, &get);
        if blocks.is_empty() {
            self.status = Some(("The page has no text or pictures the reader can put in a PDF.".into(), false));
            return;
        }
        let dir = match Self::downloads_dir(cx) {
            Ok(d) => d,
            Err(e) => {
                self.status = Some((e, false));
                return;
            }
        };
        let title = if page.doc.title.trim().is_empty() { page.url.to_string() } else { page.doc.title.clone() };
        let size = self.pdf_size();
        let mut b = iyakki_pdf::Builder::new(size);
        b.title = title.clone();
        b.blocks = blocks;
        b.date = Some(iyakki_agent::pdftools::pdf_date(iyakki_agent::clock::now_unix()));
        let (bytes, built) = b.build();
        let path = iyakki_agent::pdftools::unique_in(&dir, &iyakki_agent::pdftools::safe_name(&title), "pdf");
        if let Err(e) = iyakki_agent::scope::check_write(&cx.session.policy, &cx.session.user, &path) {
            self.status = Some((e, false));
            return;
        }
        match std::fs::write(&path, &bytes) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "browser.save_pdf", &page.url.host);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let note = if built.replaced_chars > 0 { format!(" {} characters of other scripts were written as ?.", built.replaced_chars) } else { String::new() };
                self.status = Some((format!("Saved {name} to Downloads ({} page{}, {}).{note}", built.pages, if built.pages == 1 { "" } else { "s" }, if size == PageSize::A4 { "A4" } else { "Letter" }), true));
            }
            Err(e) => self.status = Some((format!("Could not save the PDF: {e}"), false)),
        }
    }

    pub(super) fn toggle_pdf_size(&mut self, cx: &AppCx) {
        let next = if self.pdf_size() == PageSize::A4 { "Letter" } else { "A4" };
        self.data.pdf_size = next.into();
        browser_data::save(cx, &self.data);
        self.status = Some((format!("Pages saved as PDF are now {next}."), true));
    }

    /// Route an event to the open viewer. `Some(flow)`: the viewer took it.
    pub(super) fn pdf_event(&mut self, ev: &Event) -> Option<Flow> {
        let v = self.pdf.as_mut()?;
        let mine = match ev {
            Event::Tab(n) => super::super::pdfview::is_tab(*n),
            Event::Focus(n) | Event::FocusAt(n, _) => super::super::pdfview::is_tab(*n),
            // the viewer's own chords; every other Ctrl chord (tabs, address, downloads) stays the browser's
            Event::Key(Key::Ctrl(c)) => matches!(c, 'f' | 'F' | 'c'),
            Event::Key(Key::F(_)) | Event::Key(Key::Tab) | Event::Key(Key::BackTab) => false,
            Event::Key(_) | Event::Scroll(_) | Event::ScrollPx(_) | Event::Paste(_) => true,
            _ => false,
        };
        if !mine {
            return None;
        }
        match v.handle(ev) {
            Flow::Back => {
                self.pdf = None;
                Some(Flow::Stay)
            }
            Flow::Stay => Some(Flow::Stay),
        }
    }
}
