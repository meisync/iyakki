//! The shaped-script writer (F-PDF-15) checked by programs that are not ours and are never shipped: poppler's `pdftotext`, `pdffonts`
//! and `pdftoppm`, and Ghostscript. Each test is skipped, and says so, where a tool is missing. The checks: the file opens and
//! renders with no complaint, the fonts are embedded subsets, the text copied out is the text typed (logical order, through
//! `/ActualText`), and the ink of a rendered page sits where the layout says (left margin for left-to-right paragraphs, right margin for
//! right-to-left ones).

use crate::write::{Block, Builder, PageSize};
use std::process::Command;

fn have(tool: &str, arg: &str) -> bool {
    Command::new(tool).arg(arg).output().is_ok()
}

fn tools() -> bool {
    let ok = have("pdftotext", "-v") && have("pdffonts", "-v") && have("pdftoppm", "-v") && have("gs", "--version");
    if !ok {
        eprintln!("skipped: pdftotext, pdffonts, pdftoppm or gs is missing");
    }
    ok
}

fn dir(tag: &str) -> std::path::PathBuf {
    // /tmp, not the build's TMPDIR: this Ghostscript cannot open output files under a dot folder of the home
    let d = std::path::PathBuf::from("/tmp").join(format!("iyakki-shaped-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn make(blocks: Vec<Block>) -> (Vec<u8>, usize) {
    let mut b = Builder::new(PageSize::A4);
    b.title = "Shaped".into();
    b.blocks = blocks;
    let (bytes, built) = b.build();
    (bytes, built.replaced_chars)
}

fn text_of(d: &std::path::Path, bytes: &[u8]) -> String {
    let f = d.join("t.pdf");
    std::fs::write(&f, bytes).unwrap();
    let o = Command::new("pdftotext").arg(&f).arg("-").output().unwrap();
    assert!(o.status.success(), "pdftotext: {}", String::from_utf8_lossy(&o.stderr));
    assert!(o.stderr.is_empty(), "pdftotext complained: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && *c != '\u{200e}' && *c != '\u{200f}').collect()
}

/// A grey page at 72 dpi: (width, height, pixels).
fn render(d: &std::path::Path, bytes: &[u8]) -> (usize, usize, Vec<u8>) {
    let f = d.join("r.pdf");
    std::fs::write(&f, bytes).unwrap();
    if let Ok(k) = std::env::var("IYAKKI_DUMP_PDF") {
        let _ = std::fs::copy(&f, k);
    }
    let o = Command::new("gs").env("TMPDIR", "/tmp").args(["-q", "-dNOPAUSE", "-dBATCH", "-dSAFER", "-sDEVICE=pgmraw", "-r72", "-dFirstPage=1", "-dLastPage=1"]).arg(format!("-sOutputFile={}", d.join("g.pgm").display())).arg(&f).output().unwrap();
    assert!(o.status.success() && o.stderr.is_empty() && o.stdout.is_empty(), "gs: {} {}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let p = std::fs::read(d.join("g.pgm")).unwrap();
    // P5 <w> <h> 255\n
    let mut parts = Vec::new();
    let mut i = 0;
    while parts.len() < 4 {
        while p[i].is_ascii_whitespace() {
            i += 1;
        }
        if p[i] == b'#' {
            while p[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let s = i;
        while !p[i].is_ascii_whitespace() {
            i += 1;
        }
        parts.push(String::from_utf8_lossy(&p[s..i]).into_owned());
    }
    i += 1;
    (parts[1].parse().unwrap(), parts[2].parse().unwrap(), p[i..].to_vec())
}

/// Left and right edge (pixels) of the ink in rows `y0..y1`.
fn ink_x(w: usize, px: &[u8], y0: usize, y1: usize) -> Option<(usize, usize)> {
    let (mut lo, mut hi) = (usize::MAX, 0usize);
    for y in y0..y1 {
        for x in 0..w {
            if px[y * w + x] < 140 {
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
    }
    (hi >= lo && lo != usize::MAX).then_some((lo, hi))
}

#[test]
fn indic_and_thai_text_embeds_noto_subsets_and_copies_out_as_typed() {
    if !tools() {
        return;
    }
    let d = dir("indic");
    let samples = [("हिन्दी भाषा सुंदर है", "NotoSansDevanagari"), ("தமிழ் மொழி இனிமையானது கெட்டி", "NotoSansTamil"), ("ภาษาไทยที่สวยงาม", "NotoSansThai"), ("বাংলা ভাষা", "NotoSansBengali"), ("తెలుగు భాష", "NotoSansTelugu")];
    let blocks: Vec<Block> = std::iter::once(Block::Heading(1, "Scripts".into())).chain(samples.iter().map(|(t, _)| Block::Para(format!("Latin and {t} mixed."))) ).collect();
    let (bytes, replaced) = make(blocks);
    assert_eq!(replaced, 0);
    let out = text_of(&d, &bytes);
    let flat = squash(&out);
    for (t, _) in &samples {
        assert!(flat.contains(&squash(t)), "{t} not found in {out:?}");
    }
    let f = d.join("t.pdf");
    let fonts = Command::new("pdffonts").arg(&f).output().unwrap();
    let listing = String::from_utf8_lossy(&fonts.stdout);
    for (_, name) in &samples {
        let row = listing.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{name} missing from\n{listing}"));
        assert!(row.contains("yes yes"), "embedded and subset: {row}");
    }
    render(&d, &bytes);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn hebrew_and_arabic_copy_out_in_logical_order_and_sit_against_the_right_margin() {
    if !tools() {
        return;
    }
    let d = dir("rtl");
    let heb = "שלום עולם, זה טקסט בעברית";
    let ara = "مرحبا بالعالم هذا نص عربي";
    let (bytes, replaced) = make(vec![Block::Para(heb.into()), Block::Para("The next paragraph is English and runs left to right.".into()), Block::Para(ara.into())]);
    assert_eq!(replaced, 0);
    let out = text_of(&d, &bytes);
    let flat = squash(&out);
    // poppler lists the words of a right-to-left line from the left of the page, so the check is word by word: every word
    // comes back with its letters in the order typed (the glyphs sit in visual order in the file)
    for w in heb.split_whitespace().chain(ara.split_whitespace()) {
        let w = w.trim_end_matches(',');
        assert!(flat.contains(w), "{w} not found in {out:?}");
    }
    // ink: first line (Hebrew) hugs the right margin, the English line the left one; A4 at 72 dpi is 595 wide, margins 54
    let (w, _h, px) = render(&d, &bytes);
    let top = 54usize; // the first line starts at the top margin
    let heb_ink = ink_x(w, &px, top, top + 16).expect("Hebrew line has ink");
    assert!((540..=544).contains(&heb_ink.1) || (538..=545).contains(&heb_ink.1), "Hebrew right edge {heb_ink:?}");
    assert!(heb_ink.0 > 300, "Hebrew starts far from the left margin: {heb_ink:?}");
    let eng_ink = ink_x(w, &px, top + 20, top + 36).expect("English line has ink");
    assert!((52..=58).contains(&eng_ink.0), "English left edge {eng_ink:?}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_long_hindi_paragraph_wraps_inside_the_margins_over_pages() {
    if !tools() {
        return;
    }
    let d = dir("wrap");
    let sentence = "यह एक लंबा वाक्य है जो कई पंक्तियों में बँटना चाहिए ताकि हर पंक्ति पन्ने की चौड़ाई में बैठे। ";
    let (bytes, _) = make(vec![Block::Para(sentence.repeat(60))]);
    let out = text_of(&d, &bytes);
    assert!(squash(&out).matches("बँटना").count() >= 55, "words found: {}", squash(&out).matches("बँटना").count());
    let (w, _h, px) = render(&d, &bytes);
    let (lo, hi) = ink_x(w, &px, 0, px.len() / w).unwrap();
    assert!(lo >= 50 && hi <= 546, "ink {lo}..{hi} must stay inside the 54 pt margins");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn letters_no_font_has_are_question_marks_and_counted_and_the_file_is_still_sound() {
    if !tools() {
        return;
    }
    let d = dir("unk");
    let (bytes, replaced) = make(vec![Block::Para("नमस्ते 日本語 end".into())]);
    assert_eq!(replaced, 3);
    let out = text_of(&d, &bytes);
    assert!(squash(&out).contains("नमस्ते???end"), "{out:?}");
    render(&d, &bytes);
    let _ = std::fs::remove_dir_all(d);
}

// ---------------------------------------------------------------------------------------------- F-PDF-16

fn dark(w: usize, px: &[u8]) -> usize {
    let _ = w;
    px.iter().filter(|&&p| p < 128).count()
}

#[test]
fn a_heading_in_a_shaped_script_is_heavier_than_the_same_words_in_a_paragraph() {
    // the bundled fonts have one weight; the heading is filled and stroked, so its letters carry more ink
    let text = "हिन्दी भाषा सुंदर है";
    if !tools() {
        return;
    }
    // same size for both, so the only difference is the weight
    let size_of = |b: &[u8]| {
        let d = dir("bold");
        let (w, _h, px) = render(&d, b);
        let r = (w, dark(w, &px));
        let _ = std::fs::remove_dir_all(d);
        r
    };
    let mut hb = Builder::new(PageSize::A4);
    hb.footer = false;
    hb.blocks = vec![Block::Heading(6, text.into())]; // level 6: 11 pt, as the paragraph
    let heavy = hb.build().0;
    let mut pb = Builder::new(PageSize::A4);
    pb.footer = false;
    pb.blocks = vec![Block::Para(text.into())];
    let light = pb.build().0;
    let (_, d_heavy) = size_of(&heavy);
    let (_, d_light) = size_of(&light);
    assert!(d_heavy * 100 > d_light * 112, "heavy {d_heavy} light {d_light}");
    // and the words still copy out as typed
    let d = dir("boldtext");
    assert!(squash(&text_of(&d, &heavy)).contains(&squash(text)));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_title_in_a_shaped_script_is_in_the_footer_and_in_the_document_information() {
    let mut b = Builder::new(PageSize::A4);
    b.title = "ภาษาไทย และ हिन्दी".into();
    b.blocks = vec![Block::Para("Body.".into()), Block::PageBreak, Block::Para("More.".into())];
    let (bytes, built) = b.build();
    assert_eq!(built.replaced_chars, 0);
    assert_eq!(built.pages, 2);
    if !tools() {
        return;
    }
    let d = dir("footer");
    let out = text_of(&d, &bytes);
    let flat = squash(&out);
    assert!(flat.matches(&squash("ภาษาไทย")).count() >= 2, "the Thai title is on both pages: {out:?}");
    assert!(flat.matches(&squash("हिन्दी")).count() >= 2);
    assert!(flat.contains("1/2") && flat.contains("2/2"));
    render(&d, &bytes);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn thai_without_spaces_wraps_between_words_and_the_copy_is_the_text_typed() {
    let sentence = "ฉันกินข้าวกับปลาที่ตลาดวันนี้ แล้วเขาก็บอกว่าไม่รู้เรื่อง ไม่เข้าใจคำถามของเธอเลย ภาษาไทยเป็นภาษาที่สวยงามมาก ";
    let text = sentence.repeat(6);
    let mut replaced = 0;
    let lines = crate::shaped::layout(&text, 11.0, 200.0, &mut replaced);
    assert_eq!(replaced, 0);
    assert!(lines.len() > 6);
    let all: String = lines.iter().map(|l| l.clusters.iter().map(|c| c.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("");
    assert_eq!(squash(&all), squash(&text), "no letter lost");
    // the cuts fall between pieces the segmenter finds: every line, glued to the next, keeps the piece boundary at the join
    let mut at_boundary = 0;
    for pair in lines.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        assert!(a.width <= 200.0 + 0.5);
        let last: String = a.clusters.iter().map(|c| c.text.as_str()).collect();
        let first: String = b.clusters.iter().map(|c| c.text.as_str()).collect();
        let (Some(lc), Some(fc)) = (last.chars().last(), first.chars().next()) else { continue };
        if crate::thai::is_thai(lc) && crate::thai::is_thai(fc) {
            let glued: Vec<char> = format!("{last}{first}").chars().collect();
            let cut = last.chars().count();
            let breaks = crate::thai::run_breaks(&glued);
            // a cut that the dictionary does not make is allowed only where the spelling rules allow a cut (never before a mark)
            assert!(!matches!(fc, '\u{0E30}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}'), "line starts with a mark: {first}");
            if breaks.contains(&cut) {
                at_boundary += 1;
            }
        }
    }
    assert!(at_boundary * 10 >= lines.len() * 6, "most cuts are at words: {at_boundary} of {}", lines.len());
    if !tools() {
        return;
    }
    let d = dir("thaiwrap");
    let (bytes, replaced) = make(vec![Block::Para(text.clone())]);
    assert_eq!(replaced, 0);
    assert!(squash(&text_of(&d, &bytes)).contains(&squash(sentence)));
    render(&d, &bytes);
    let _ = std::fs::remove_dir_all(d);
}

// ---------------------------------------------------------------------------------------------- F-PDF-18: right to left, read back

#[test]
fn right_to_left_lines_are_read_back_in_the_order_they_were_typed() {
    use crate::Document;
    for text in ["שלום עולם, זה טקסט בעברית", "مرحبا بالعالم هذا نص عربي", "שלום Hello 123 עולם"] {
        let (bytes, _) = make(vec![Block::Para(text.into())]);
        let doc = Document::open(bytes).unwrap();
        let got = doc.page_text(0).unwrap();
        assert!(squash(&got).contains(&squash(text)), "{text:?} read back as {got:?}");
    }
    // a line of Latin text is not touched
    let (bytes, _) = make(vec![Block::Para("Plain English, 123.".into())]);
    assert!(Document::open(bytes).unwrap().page_text(0).unwrap().contains("Plain English, 123."));
    assert_eq!(crate::text::visual_to_logical("abc 123"), "abc 123");
}
