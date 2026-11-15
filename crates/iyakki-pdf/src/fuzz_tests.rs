//! Fuzz tests (deterministic, seeded) for the PDF reader, the Markdown reader, the TrueType reader/subsetter and the embedded-font writer.
//! A panic, a case slower than the guard or a case over the heap bound fails the test and prints the seed and case.
//! `IYAKKI_FUZZ_CASES=50000 cargo test -p iyakki-pdf fuzz` runs a larger set.

use crate::fuzzkit::{cases, run, Rng};
use crate::tests_support::{objstm_file, simple};
use crate::ttf::Ttf;
use crate::{Block, BoxGlyphs, Builder, Document, PageSize, PdfImage};
use std::collections::BTreeSet;
use std::time::Duration;

fn rich_pdf() -> Vec<u8> {
    let px: Vec<u8> = (0..30 * 30).flat_map(|i| [(i % 251) as u8, 40, 200, if i % 7 == 0 { 0 } else { 255 }]).collect();
    let mut b = Builder::new(PageSize::A4);
    b.title = "Fuzz \u{3a3}\u{3b1}".into();
    b.author = "someone".into();
    b.blocks = vec![
        Block::Heading(1, "Heading one".into()),
        Block::Para("A paragraph with Greek \u{39a}\u{3b1}\u{3bb}\u{3b7}\u{3bc}\u{3ad}\u{3c1}\u{3b1} and Cyrillic \u{41f}\u{440}\u{438}\u{432}\u{435}\u{442} text. ".repeat(6)),
        Block::Bullet(0, "\u{2022}".into(), "item".into()),
        Block::Quote("quoted".into()),
        Block::Code("fn main() {}\n  indented".into()),
        Block::Rule,
        Block::Image(PdfImage::from_rgba(30, 30, &px).unwrap()),
        Block::PageBreak,
        Block::Para("Second page".into()),
    ];
    b.build().0
}

/// Pages with the content-stream operators a reader must survive: nesting of q/Q, text state, paths, inline images, hostile numbers.
fn operator_pdf() -> Vec<u8> {
    let content = "q 1 0 0 1 10 10 cm BT /F1 12 Tf 0 0 Td (Hello \\(w\\) orld) Tj [(a) -120 (b)] TJ T* ET Q\n0 0 100 50 re f\n10 10 m 50 50 l 90 10 l h S\n1 0 0 rg 0 1 0 RG 2 w [3 2] 0 d\nBI /W 2 /H 2 /CS /G /BPC 8 ID \u{1}\u{2}\u{3}\u{4} EI\n/Fm0 Do q q q Q Q Q 99999999999 0 0 99999999999 0 0 cm BT (x) Tj ET";
    simple(&[content, "BT /F1 9 Tf (second) Tj ET"])
}

#[test]
fn fuzz_pdf_reader_never_panics_hangs_or_overgrows() {
    let corpus = vec![simple(&["BT /F1 12 Tf 10 50 Td (Hello fuzz) Tj ET"]), objstm_file("BT /F1 12 Tf 10 50 Td (Hello objstm) Tj ET"), rich_pdf(), operator_pdf()];
    let n = run("pdf reader", 0x9DF, &corpus, 450, Duration::from_secs(5), 768 << 20, |input| {
        let Ok(doc) = Document::open(input.to_vec()) else { return };
        let _ = doc.info();
        let _ = doc.text(100_000);
        let _ = doc.form_fields();
        let _ = doc.search("e", false);
        let pages = doc.page_count().min(3);
        for i in 0..pages {
            let _ = doc.page_lines(i);
            let _ = doc.page_chars(i);
            let _ = doc.page_size(i);
            let _ = doc.render(i, 0.5, &BoxGlyphs);
        }
        let _ = doc.page(doc.page_count());
        let _ = doc.render(0, f64::NAN, &BoxGlyphs);
    });
    assert!(n >= cases(1000));
}

#[test]
fn fuzz_pdf_reader_with_repaired_cross_references_reaches_the_object_parsers() {
    // mutate the body only, so the table still opens the file: rewrite the cross-reference by scanning is the reader's own repair path
    let base = vec![simple(&["BT /F1 12 Tf 10 50 Td (Hello) Tj ET 0 0 50 50 re f"]), rich_pdf()];
    let mut r = Rng::new(0x9DF2);
    for k in 0..cases(500) {
        let pick = r.pick(&base).clone();
        let mut m = crate::fuzzkit::mutate(&mut r, &pick);
        // damage the startxref so that the repair scan has to find the objects
        if r.chance(2) {
            if let Some(p) = m.windows(9).rposition(|w| w == b"startxref") {
                m.truncate(p);
            }
        }
        let t = std::time::Instant::now();
        if let Ok(doc) = Document::open(m.clone()) {
            let _ = doc.text(10_000);
            let _ = doc.render(0, 0.4, &BoxGlyphs);
        }
        assert!(t.elapsed() < Duration::from_secs(5), "case {k} took {:?}", t.elapsed());
    }
}

fn fonts() -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    for ix in 0..3 {
        let f = crate::embed::face(ix).unwrap();
        let chars: Vec<(u32, u16)> = "Hello \u{3a3}\u{3b1}\u{41f}\u{e9}\u{2014}fi\u{fb01}".chars().map(|c| (c as u32, f.gid(c))).collect();
        let keep = f.ttf.closure(&chars.iter().map(|c| c.1).collect());
        v.push(f.ttf.subset(&keep, &chars));
    }
    v
}

fn exercise_font(input: &[u8]) {
    let Some(t) = Ttf::parse(input) else { return };
    let map = t.cmap_map();
    let _ = (t.ascent(), t.descent(), t.bbox());
    let mut want: BTreeSet<u16> = [0u16, 1, 2, 65535, t.num_glyphs.wrapping_sub(1), t.num_glyphs].into_iter().collect();
    want.extend(map.values().take(40).copied());
    for g in &want {
        let _ = t.advance(*g);
        let _ = t.outline_len(*g);
    }
    let keep = t.closure(&want);
    assert!(keep.len() <= t.num_glyphs as usize + 1);
    let chars: Vec<(u32, u16)> = map.iter().take(60).map(|(c, g)| (*c, *g)).collect();
    let out = t.subset(&keep, &chars);
    assert!(out.len() < 64 << 20, "subset grew to {} bytes", out.len());
}

#[test]
fn fuzz_ttf_parser_and_subsetter_on_mutated_fonts() {
    let mut corpus = fonts();
    let n = run("ttf subsets", 0x77F, &corpus, 6000, Duration::from_secs(5), 256 << 20, exercise_font);
    assert!(n >= cases(6000));
    // the real fonts: the table directory and the small tables are mutated, the big glyph data is left as it is
    let real = crate::embed::face(0).unwrap();
    corpus.clear();
    let _ = real;
    let sans = include_bytes!("../../iyakki-shell/assets/DejaVuSans.ttf");
    let mut r = Rng::new(0x77F2);
    for _ in 0..cases(300) {
        let head = crate::fuzzkit::mutate(&mut r, &sans[..600]);
        let mut m = head;
        m.extend_from_slice(&sans[600..]);
        let t = std::time::Instant::now();
        exercise_font(&m);
        assert!(t.elapsed() < Duration::from_secs(8), "slow font case: {:?}", t.elapsed());
    }
}

#[test]
fn fuzz_markdown_to_blocks_to_pdf() {
    let md = "# Title\n\nFirst line\nsecond line.\n\n- one\n  - nested\n1. numbered\n\n> a wise\n> saying\n\n---\n\n```\nlet x = 1;\n```\n\n| a | bb |\n|---|----|\n| 1 | 22 |\n\nWith **bold**, *italic*, `code` and [a link](https://e.org/x) ![pic](a.png) \u{3a3}\u{3b1}.\n";
    let corpus = vec![md.as_bytes().to_vec(), "> > > > > > quote\n".repeat(30).into_bytes(), "- a\n  - b\n    - c\n      - d\n".repeat(20).into_bytes(), "|a|b|\n|-|-|\n|1|2|\n".as_bytes().to_vec(), "**[`*_x_*`](((**\n".repeat(40).into_bytes()];
    let n = run("markdown", 0x3D, &corpus, 700, Duration::from_secs(5), 256 << 20, |input| {
        let text = String::from_utf8_lossy(input).into_owned();
        let _ = crate::md::strip_inline(&text);
        let _ = crate::md::fold(&text);
        let mut asked = 0;
        let blocks = crate::md::markdown_blocks(&text, &mut |_: &str| {
            asked += 1;
            let px: Vec<u8> = (0..8 * 8).flat_map(|_| [10, 200, 30, 255]).collect();
            PdfImage::from_rgba(8, 8, &px).ok_or_else(|| "no".to_string())
        });
        if let Ok(blocks) = blocks {
            let _ = crate::md::outside_latin("t", &blocks, 5);
            let mut b = Builder::new(PageSize::A4);
            b.blocks = blocks;
            let (pdf, built) = b.build();
            assert!(built.pages >= 1);
            // the reader opens what the writer wrote, whatever the text was
            let doc = Document::open(pdf).expect("a PDF this writer made must open");
            assert_eq!(doc.page_count(), built.pages);
        }
    });
    assert!(n >= cases(1200));
}

#[test]
fn fuzz_ttf_regression_tables_shorter_than_the_subsetter_patches_are_refused() {
    // a font whose `head` table is 0 bytes long used to parse and then panic in `subset` (slice out of range)
    for mut f in fonts() {
        let n = u16::from_be_bytes([f[4], f[5]]) as usize;
        for i in 0..n {
            let e = 12 + i * 16;
            if &f[e..e + 4] == b"head" || &f[e..e + 4] == b"hhea" || &f[e..e + 4] == b"maxp" {
                f[e + 12..e + 16].copy_from_slice(&0u32.to_be_bytes());
            }
        }
        assert!(Ttf::parse(&f).is_none());
    }
}
