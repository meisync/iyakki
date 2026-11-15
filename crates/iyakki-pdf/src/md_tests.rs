//! Markdown-ish text to blocks to a PDF and back through the reader.

use crate::md::*;
use crate::{Block, BoxGlyphs, Builder, Document, PageSize, PdfImage};

fn no_pictures(_: &str) -> Result<PdfImage, String> {
    Err("no pictures here".into())
}

fn red() -> PdfImage {
    let px: Vec<u8> = (0..60 * 60).flat_map(|_| [255, 0, 0, 255]).collect();
    PdfImage::from_rgba(60, 60, &px).unwrap()
}

#[test]
fn each_kind_of_line_becomes_its_block() {
    let md = "# Title\n\nFirst line\nsecond line of the same paragraph.\n\n- one\n  - nested\n1. numbered\n\n> a wise\n> saying\n\n---\n\n```\nlet x = 1;\n  indented\n```\n\n| a | bb |\n|---|----|\n| 1 | 22 |\n\nWith **bold**, *italic*, `code` and [a link](https://e.org/x).";
    let b = markdown_blocks(md, &mut no_pictures).unwrap();
    assert_eq!(b[0], Block::Heading(1, "Title".into()));
    assert_eq!(b[1], Block::Para("First line second line of the same paragraph.".into()));
    assert_eq!(b[2], Block::Bullet(0, "\u{2022}".into(), "one".into()));
    assert_eq!(b[3], Block::Bullet(1, "\u{2022}".into(), "nested".into()));
    assert_eq!(b[4], Block::Bullet(0, "1.".into(), "numbered".into()));
    assert_eq!(b[5], Block::Quote("a wise saying".into()));
    assert_eq!(b[6], Block::Rule);
    assert_eq!(b[7], Block::Code("let x = 1;\n  indented".into()));
    assert!(matches!(&b[8], Block::Code(t) if t.lines().count() == 2 && t.contains("a  bb") && t.contains("1  22")), "{:?}", b[8]);
    assert_eq!(b[9], Block::Para("With bold, italic, code and a link (https://e.org/x).".into()));
}

#[test]
fn a_stray_star_or_underscore_stays_and_look_alikes_are_folded() {
    assert_eq!(strip_inline("5 * 3 = 15 and snake_case_name"), "5 * 3 = 15 and snake_case_name");
    assert_eq!(fold("a \u{2192} b \u{2265} c \u{2212}1"), "a -> b >= c -1");
}

#[test]
fn pictures_are_asked_for_by_address_and_a_missing_one_stops_the_conversion() {
    let mut asked = Vec::new();
    let b = markdown_blocks("Before\n\n![a fox](pics/01.png)\n\nAfter", &mut |a: &str| {
        asked.push(a.to_string());
        Ok(red())
    })
    .unwrap();
    assert_eq!(asked, vec!["pics/01.png"]);
    assert!(matches!(b[1], Block::Image(_)));
    let e = markdown_blocks("![x](/etc/passwd)", &mut |a: &str| Err(format!("{a}: not allowed"))).unwrap_err();
    assert!(e.contains("/etc/passwd"));
    let many = "![x](a.png)\n".repeat(MAX_PICTURES + 1);
    assert!(markdown_blocks(&many, &mut |_: &str| Ok(red())).unwrap_err().contains("too many pictures"));
}

#[test]
fn text_outside_windows_1252_is_found_and_counted() {
    let b = vec![Block::Para("caf\u{e9} \u{2014} fine, but \u{0ba4}\u{0bae}\u{0bbf}\u{0bb4}\u{0bcd} is not".into()), Block::Heading(2, "\u{3053}\u{3093}".into())];
    let (n, seen) = outside_latin("Title", &b, 3);
    assert_eq!(n, 7, "five Tamil code points and two Japanese");
    assert_eq!(seen.len(), 3);
    assert_eq!(outside_latin("ok", &[Block::Para("caf\u{e9} \u{201c}quoted\u{201d} \u{20ac}".into())], 3).0, 0);
}

#[test]
fn the_written_file_reads_back_with_its_text_pages_and_picture() {
    let md = "# Report\n\nIntro words.\n\n![chart](c.png)\n\n- first point\n- second point\n";
    let blocks = markdown_blocks(md, &mut |_: &str| Ok(red())).unwrap();
    let mut b = Builder::new(PageSize::Letter);
    b.title = "Report".into();
    b.footer = false;
    b.blocks = blocks;
    let (bytes, built) = b.build();
    assert_eq!(built.replaced_chars, 0);
    let d = Document::open(bytes).unwrap();
    assert_eq!(d.page_count(), 1);
    let t = d.page_text(0).unwrap();
    assert!(t.contains("Intro words.") && t.contains("second point"), "{t}");
    let r = d.render(0, 1.0, &BoxGlyphs).unwrap();
    let has_red = r.rgba.chunks(4).any(|p| p[0] > 240 && p[1] < 20 && p[2] < 20);
    assert!(has_red, "the picture is drawn");
}

#[test]
fn a_page_break_starts_a_new_page_and_does_nothing_at_the_top_of_a_page() {
    let page = |blocks: Vec<Block>| {
        let mut b = Builder::new(PageSize::A4);
        b.blocks = blocks;
        let (bytes, built) = b.build();
        (Document::open(bytes).unwrap(), built.pages)
    };
    let (d, n) = page(vec![Block::Para("first words".into()), Block::PageBreak, Block::PageBreak, Block::Para("second words".into())]);
    assert_eq!(n, 2, "two breaks in a row make one new page");
    assert!(d.page_text(0).unwrap().contains("first words") && !d.page_text(0).unwrap().contains("second") && d.page_text(1).unwrap().contains("second words"));
    assert_eq!(page(vec![Block::PageBreak, Block::Para("only".into())]).1, 1, "no empty page before the first block");
}

#[test]
fn a_long_line_of_opening_marks_costs_time_in_proportion_to_its_length_and_short_marks_still_work() {
    // Proportion, not wall clock: a loaded machine slows both runs alike, a quadratic scan makes 4x the input cost ~16x the time.
    let run = |unit: &str, chars: usize| {
        let text = unit.repeat(chars / unit.len());
        let t = std::time::Instant::now();
        let blocks = markdown_blocks(&text, &mut no_pictures).unwrap();
        assert!(!blocks.is_empty());
        t.elapsed().max(std::time::Duration::from_millis(30))
    };
    for unit in ["[", "*a ", "_a ", "[a]("] {
        let small = run(unit, 20_000);
        let big = run(unit, 80_000);
        assert!(big < small * 10, "{unit:?}: 4x the input took {big:?} against {small:?}, which is not in proportion");
    }
    assert_eq!(strip_inline("see [the page](https://example.org/x) and **bold** and *it*"), "see the page (https://example.org/x) and bold and it");
}
