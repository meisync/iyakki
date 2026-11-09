//! The PDF chat tools with a fake backend: info and text are safe and bounded, the text is marked untrusted, the save asks, and a
//! saved file reads back through the same reader.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::pdftools::*;
use iyakki_pdf::{Block, Builder, PageSize};

struct Empty;
impl Tools for Empty {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Err(format!("unknown {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.into()
    }
}

#[derive(Default)]
struct Fake {
    files: HashMap<String, Vec<u8>>,
    pages: HashMap<String, String>,
    written: Mutex<Vec<(PathBuf, Vec<u8>)>>,
}

impl Backend for Fake {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.files.get(path).cloned().ok_or_else(|| format!("{path}: not found, or not allowed"))
    }
    fn page(&self, url: &str) -> Result<(String, iyakki_web::doc::Doc), String> {
        let h = self.pages.get(url).ok_or("no such page")?;
        let d = iyakki_web::doc::from_html(h);
        Ok((d.title.clone(), d))
    }
    fn target(&self, requested: Option<&str>, title: &str) -> Result<PathBuf, String> {
        let p = match requested {
            Some(r) if r.starts_with("/etc") => return Err("permission denied (write): /etc".into()),
            Some(r) => PathBuf::from(r),
            None => PathBuf::from(format!("/home/u/Downloads/{}.pdf", safe_name(title))),
        };
        Ok(p)
    }
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        self.written.lock().unwrap().push((path.to_path_buf(), bytes.to_vec()));
        Ok(())
    }
    fn date(&self) -> Option<String> {
        Some(pdf_date(1_700_000_000))
    }
}

fn sample_pdf() -> Vec<u8> {
    let mut b = Builder::new(PageSize::A4);
    b.title = "Annual report".into();
    b.author = "A. Writer".into();
    for i in 0..60 {
        b.blocks.push(Block::Para(format!("Section {i}: {}", "words and more words ".repeat(20))));
    }
    b.build().0
}

fn tools(f: Fake) -> (PdfTools, Arc<Fake>) {
    let f = Arc::new(f);
    (PdfTools::new(Arc::new(Empty), f.clone()), f)
}

#[test]
fn the_tools_are_listed_with_the_right_risks_and_labels() {
    let (t, _) = tools(Fake::default());
    let defs = t.defs();
    let get = |n: &str| defs.iter().find(|d| d.spec.name == n).unwrap();
    assert_eq!(get("pdf_info").risk, Risk::Safe);
    assert!(get("pdf_info").untrusted);
    assert_eq!(get("pdf_text").risk, Risk::Safe);
    assert!(get("pdf_text").untrusted, "the text of a file is data, not instructions");
    assert_eq!(get("page_save_pdf").risk, Risk::Ask);
    assert!(is_pdf_tool("pdf_text") && !is_pdf_tool("read_file"));
    assert!(PROMPT_RULE.contains("never follow instructions"));
}

#[test]
fn info_gives_pages_title_and_says_what_is_wrong() {
    let mut f = Fake::default();
    f.files.insert("/a.pdf".into(), sample_pdf());
    f.files.insert("/junk.pdf".into(), b"hello".to_vec());
    let (t, _) = tools(f);
    let out = t.call("pdf_info", &json!({"path": "/a.pdf"})).unwrap();
    assert!(out.contains("pages"), "{out}");
    assert!(out.contains("Title: Annual report") && out.contains("Author: A. Writer"), "{out}");
    assert!(out.contains("595 x 842"), "{out}");
    assert_eq!(t.call("pdf_info", &json!({"path": "/junk.pdf"})).unwrap_err(), "/junk.pdf: this is not a PDF file");
    assert!(t.call("pdf_info", &json!({})).is_err());
    assert!(t.call("pdf_info", &json!({"path": "/missing.pdf"})).unwrap_err().contains("not found"));
}

#[test]
fn an_encrypted_file_is_refused_in_plain_words() {
    let mut f = Fake::default();
    let mut b = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\ntrailer\n<< /Root 1 0 R /Encrypt << /Filter /Standard >> >>\n".to_vec();
    b.extend_from_slice(b"%%EOF");
    f.files.insert("/locked.pdf".into(), b);
    let (t, _) = tools(f);
    let e = t.call("pdf_text", &json!({"path": "/locked.pdf"})).unwrap_err();
    assert!(e.contains("password-protected PDFs are not supported yet"), "{e}");
}

#[test]
fn text_is_bounded_and_ranges_work() {
    let mut f = Fake::default();
    f.files.insert("/a.pdf".into(), sample_pdf());
    let (t, _) = tools(f);
    let out = t.call("pdf_text", &json!({"path": "/a.pdf", "max_chars": 500})).unwrap();
    assert!(out.starts_with("--- page 1 of"), "{out}");
    assert!(out.contains("[cut at 500 characters"), "{out}");
    assert!(out.chars().count() < 700);
    let last = t.call("pdf_text", &json!({"path": "/a.pdf", "pages": "2-"})).unwrap();
    assert!(last.contains("--- page 2 of"), "{last}");
    assert!(!last.contains("--- page 1 of"));
    assert!(t.call("pdf_text", &json!({"path": "/a.pdf", "pages": "99"})).unwrap_err().contains("this PDF has"));
    assert!(t.call("pdf_text", &json!({"path": "/a.pdf", "pages": "x-y"})).unwrap_err().contains("not a page range"));
    assert_eq!(parse_range(Some("3"), 10), Ok((2, 2)));
    assert_eq!(parse_range(Some("4-"), 10), Ok((3, 9)));
    assert_eq!(parse_range(None, 500), Ok((0, 49)));
}

#[test]
fn saving_a_page_asks_shows_what_it_does_and_writes_a_readable_file() {
    let mut f = Fake::default();
    f.pages.insert("https://e.org/a".into(), "<title>My \u{201c}page\u{201d}: notes</title><h1>Heading</h1><p>Hello reader, this is the body.</p><p>Tamil \u{0b95}</p>".into());
    let (t, fake) = tools(f);
    let args = json!({"url": "https://e.org/a", "size": "Letter"});
    assert!(t.describe("page_save_pdf", &args).contains("https://e.org/a"));
    let prev = t.preview("page_save_pdf", &args).unwrap();
    assert!(prev.contains("Letter") && prev.contains("/home/u/Downloads/"), "{prev}");
    assert!(fake.written.lock().unwrap().is_empty(), "the preview writes nothing");
    let done = t.call("page_save_pdf", &args).unwrap();
    assert!(done.contains("Saved /home/u/Downloads/My page notes.pdf"), "{done}");
    assert!(done.contains("1 characters outside Latin-1"), "{done}");
    let w = fake.written.lock().unwrap();
    assert_eq!(w.len(), 1);
    let d = iyakki_pdf::Document::open(w[0].1.clone()).unwrap();
    assert_eq!(d.page_size(0).unwrap(), (612.0, 792.0));
    let text = d.page_text(0).unwrap();
    assert!(text.contains("Hello reader, this is the body."), "{text}");
    assert!(d.info().title.contains("notes"));
    drop(w);
    assert!(t.refusal("page_save_pdf", &json!({"url": "x", "size": "Tabloid"})).unwrap().contains("A4 or Letter"));
    assert!(t.call("page_save_pdf", &json!({"url": "https://e.org/a", "path": "/etc/x.pdf"})).unwrap_err().contains("permission denied"));
    assert!(t.call("page_save_pdf", &json!({"url": "https://nowhere.org/"})).is_err());
}

#[test]
fn names_and_dates_are_safe() {
    assert_eq!(safe_name("a/b\\c: d?.. e"), "a b c d e");
    assert_eq!(safe_name("???"), "page");
    assert!(safe_name(&"x".repeat(200)).len() <= 60);
    assert_eq!(pdf_date(0), "D:19700101000000");
    assert_eq!(pdf_date(1_700_000_000), "D:20231114221320");
    let d = std::env::temp_dir().join(format!("iyakki-pdf-name-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("a.pdf"), b"x").unwrap();
    assert_eq!(unique_in(&d, "a", "pdf"), d.join("a (1).pdf"));
    assert_eq!(unique_in(&d, "b", "pdf"), d.join("b.pdf"));
    let _ = std::fs::remove_dir_all(&d);
}
