//! Looks at a PDF the way the live tests need: opened with the repo's reader, then structurally (header, end, cross-reference
//! offset, nothing repaired), with the words, pictures and embedded fonts counted. Only for the ignored live tests.

#[allow(dead_code)] // read only by the ignored live tests and kept for their evidence output
pub(crate) struct Pdf {
    pub pages: usize,
    pub text: String,
    pub images: usize,
    pub embedded_fonts: usize,
    pub bytes: usize,
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

pub(crate) fn check(bytes: &[u8]) -> Result<Pdf, String> {
    if !bytes.starts_with(b"%PDF-") {
        return Err("no %PDF header".into());
    }
    let tail = String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(64)..]).to_string();
    if !tail.trim_end().ends_with("%%EOF") {
        return Err(format!("no %%EOF at the end: {tail:?}"));
    }
    let sx = tail.rfind("startxref").ok_or("no startxref")?;
    let off: usize = tail[sx + 9..].split_whitespace().next().and_then(|n| n.parse().ok()).ok_or("startxref has no offset")?;
    if off >= bytes.len() || !(bytes[off..].starts_with(b"xref") || bytes[off].is_ascii_digit()) {
        return Err(format!("startxref {off} does not point at a table"));
    }
    let d = iyakki_pdf::Document::open(bytes.to_vec()).map_err(|e| e.to_string())?;
    if d.info().repaired {
        return Err("the reader had to repair the file".into());
    }
    let mut text = String::new();
    for i in 0..d.page_count() {
        text.push_str(&d.page_text(i).map_err(|e| format!("page {}: {e}", i + 1))?);
        text.push('\n');
    }
    Ok(Pdf { pages: d.page_count(), text, images: count(bytes, b"/Subtype /Image") + count(bytes, b"/Subtype/Image"), embedded_fonts: count(bytes, b"/FontFile2"), bytes: bytes.len() })
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() > 3).map(str::to_string).collect()
}

/// How many of the longer words of `said` the PDF text has again, and how many there were.
pub(crate) fn found_back(said: &str, pdf_text: &str) -> (usize, usize) {
    let have = words(pdf_text);
    let w = words(said);
    (w.iter().filter(|x| have.contains(x)).count(), w.len())
}
