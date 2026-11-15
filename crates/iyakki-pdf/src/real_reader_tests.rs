//! The PDF writer against a reader that is not ours: macOS's own PDF engine, reached through `sips`. Skipped (and said so) where there is no `sips`.

use crate::*;
use std::process::Command;

fn have_sips() -> bool {
    Command::new("sips").arg("--help").output().is_ok()
}

fn sips_get(file: &std::path::Path, prop: &str) -> Option<f64> {
    let o = Command::new("sips").args(["-g", prop]).arg(file).output().ok()?;
    let t = String::from_utf8_lossy(&o.stdout).into_owned();
    t.lines().find_map(|l| l.trim().strip_prefix(&format!("{prop}:")).and_then(|v| v.trim().parse::<f64>().ok()))
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-pdf-real-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_file_the_writer_made_is_opened_by_the_operating_systems_pdf_engine_with_the_same_page_size_and_real_ink() {
    if !have_sips() {
        eprintln!("skipped: no sips on this machine");
        return;
    }
    let d = tmp("sips");
    let mut text = Builder::new(PageSize::A4);
    text.title = "Second reader".into();
    text.blocks.push(Block::Heading(1, "Heading one".into()));
    text.blocks.push(Block::Para("The quick brown fox jumps over the lazy dog. ".repeat(40)));
    text.blocks.push(Block::Code("fn main() {\n    println!(\"hi\");\n}".into()));
    let blank = Builder::new(PageSize::A4);
    let (tb, _) = text.build();
    let (bb, _) = blank.build();
    let (tp, bp) = (d.join("text.pdf"), d.join("blank.pdf"));
    std::fs::write(&tp, &tb).unwrap();
    std::fs::write(&bp, &bb).unwrap();
    // the system engine reads the page size we wrote (A4 is 595.28 x 841.89 points)
    let (w, h) = (sips_get(&tp, "pixelWidth"), sips_get(&tp, "pixelHeight"));
    let (w, h) = match (w, h) {
        (Some(w), Some(h)) => (w, h),
        _ => {
            eprintln!("skipped: sips cannot read PDF files here");
            let _ = std::fs::remove_dir_all(&d);
            return;
        }
    };
    assert!((w - 595.0).abs() <= 1.5 && (h - 842.0).abs() <= 1.5, "{w} x {h}");
    // and draws something: the page with text becomes a much bigger picture than the blank page does
    let (tpng, bpng) = (d.join("text.png"), d.join("blank.png"));
    for (src, out) in [(&tp, &tpng), (&bp, &bpng)] {
        let ok = Command::new("sips").args(["-s", "format", "png"]).arg(src).arg("--out").arg(out).output().is_ok_and(|o| o.status.success());
        assert!(ok && out.is_file());
    }
    let (ts, bs) = (std::fs::metadata(&tpng).unwrap().len(), std::fs::metadata(&bpng).unwrap().len());
    assert!(ts > bs * 2, "text page {ts} bytes, blank page {bs} bytes");
    // our own reader agrees on the size
    let ours = Document::open(tb).unwrap();
    let (ow, oh) = ours.page_size(0).unwrap();
    assert!((ow - w).abs() < 1.5 && (oh - h).abs() < 1.5);
    let _ = std::fs::remove_dir_all(&d);
}

/// The box that holds every pixel that is not (nearly) white: `(left, top, right, bottom)`.
fn ink_box(w: usize, h: usize, px: impl Fn(usize, usize) -> [u8; 3]) -> Option<(usize, usize, usize, usize)> {
    let (mut l, mut t, mut r, mut b) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            let c = px(x, y);
            if c.iter().any(|v| *v < 200) {
                l = l.min(x);
                r = r.max(x);
                t = t.min(y);
                b = b.max(y);
            }
        }
    }
    (l != usize::MAX).then_some((l, t, r, b))
}

/// An uncompressed Windows bitmap (24 or 32 bits, rows bottom-up or top-down) as `(width, height, pixel(x, y))`.
fn read_bmp(d: &[u8]) -> Option<(usize, usize, Vec<[u8; 3]>)> {
    let off = u32::from_le_bytes(d.get(10..14)?.try_into().ok()?) as usize;
    let w = u32::from_le_bytes(d.get(18..22)?.try_into().ok()?) as usize;
    let hraw = i32::from_le_bytes(d.get(22..26)?.try_into().ok()?);
    let bpp = u16::from_le_bytes(d.get(28..30)?.try_into().ok()?) as usize;
    let comp = u32::from_le_bytes(d.get(30..34)?.try_into().ok()?);
    if !(bpp == 24 || bpp == 32) || (comp != 0 && comp != 3) {
        return None;
    }
    let h = hraw.unsigned_abs() as usize;
    let stride = (w * bpp / 8).div_ceil(4) * 4;
    let mut out = vec![[255u8; 3]; w * h];
    for row in 0..h {
        let y = if hraw > 0 { h - 1 - row } else { row };
        for x in 0..w {
            let o = off + row * stride + x * bpp / 8;
            let px = d.get(o..o + bpp / 8)?;
            // a 32-bit bitmap has an alpha byte: what is transparent is paper
            let a = if bpp == 32 { px[3] as u32 } else { 255 };
            let over = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
            out[y * w + x] = [over(px[2]), over(px[1]), over(px[0])];
        }
    }
    Some((w, h, out))
}

#[test]
fn where_our_renderer_puts_the_ink_of_a_written_page_is_where_the_operating_systems_engine_puts_it() {
    if !have_sips() {
        eprintln!("skipped: no sips on this machine");
        return;
    }
    let d = tmp("ink");
    let mut b = Builder::new(PageSize::A4);
    b.footer = false;
    b.blocks.push(Block::Heading(1, "Heading one".into()));
    b.blocks.push(Block::Para("The quick brown fox jumps over the lazy dog and keeps running along the line. ".repeat(12)));
    b.blocks.push(Block::Rule);
    b.blocks.push(Block::Para("A last short paragraph.".into()));
    let (bytes, _) = b.build();
    let pdf = d.join("page.pdf");
    std::fs::write(&pdf, &bytes).unwrap();
    let bmp = d.join("page.bmp");
    let ok = Command::new("sips").args(["-s", "format", "bmp"]).arg(&pdf).arg("--out").arg(&bmp).output().is_ok_and(|o| o.status.success());
    let Some((sw, sh, spx)) = ok.then(|| std::fs::read(&bmp).ok()).flatten().and_then(|d| read_bmp(&d)) else {
        eprintln!("skipped: sips could not give a bitmap of the page");
        let _ = std::fs::remove_dir_all(&d);
        return;
    };
    let theirs = ink_box(sw, sh, |x, y| spx[y * sw + x]).expect("the system engine drew ink");
    let doc = Document::open(bytes).unwrap();
    let r = doc.render(0, 1.0, &BoxGlyphs).unwrap();
    let (ow, oh) = (r.width as usize, r.height as usize);
    assert!((ow as i64 - sw as i64).abs() <= 2 && (oh as i64 - sh as i64).abs() <= 2, "{ow}x{oh} against {sw}x{sh}");
    let ours = ink_box(ow, oh, |x, y| {
        let i = (y * ow + x) * 4;
        [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2]]
    })
    .expect("our renderer drew ink");
    // the first line starts at the left margin and the last one ends at the same height, to within a few pixels; the right edge depends on glyph widths
    assert!((ours.0 as i64 - theirs.0 as i64).abs() <= 6, "left {ours:?} against {theirs:?}");
    assert!((ours.1 as i64 - theirs.1 as i64).abs() <= 12, "top {ours:?} against {theirs:?}");
    assert!((ours.3 as i64 - theirs.3 as i64).abs() <= 12, "bottom {ours:?} against {theirs:?}");
    assert!((ours.2 as i64 - theirs.2 as i64).abs() <= 30, "right {ours:?} against {theirs:?}");
    let _ = std::fs::remove_dir_all(&d);
}

// ---- a PDF made by the operating system's own PDF engine (Quartz, through a small Swift program), read by our text reader, with PDFKit as the second reader.

const SWIFT_PDF: &str = r#"import Cocoa
import PDFKit
let out = CommandLine.arguments[1]
var box = CGRect(x: 0, y: 0, width: 612, height: 792)
let ctx = CGContext(URL(fileURLWithPath: out) as CFURL, mediaBox: &box, nil)!
ctx.beginPDFPage(nil)
let lines: [(String, String, CGFloat, CGFloat)] = [
    ("Helvetica Neue", "The quick brown fox jumps over the lazy dog", 14, 740),
    ("Menlo", "Monospaced line number two", 12, 710),
    ("Times New Roman", "Καλημέρα κόσμε Greek in Times", 14, 680),
    ("Arial", "Привет мир Cyrillic in Arial", 14, 650),
    ("Helvetica", "Plain Helvetica base font line", 14, 620),
]
for (f, t, s, y) in lines {
    let font = NSFont(name: f, size: s) ?? NSFont.systemFont(ofSize: s)
    let a = NSAttributedString(string: t, attributes: [.font: font])
    let line = CTLineCreateWithAttributedString(a)
    ctx.textPosition = CGPoint(x: 72, y: y)
    CTLineDraw(line, ctx)
}
ctx.endPDFPage()
ctx.closePDF()
if let d = PDFDocument(url: URL(fileURLWithPath: out)), let s = d.string { print(s.precomposedStringWithCanonicalMapping) }
"#;

/// Whitespace made plain, and the micro sign (U+00B5) that PDFKit returns for a Greek mu (U+03BC) made the same letter.
fn squash(s: &str) -> String {
    s.replace('\u{b5}', "\u{3bc}").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn a_pdf_made_by_the_operating_systems_engine_with_embedded_fonts_is_read_as_the_same_text_by_us_and_by_pdfkit() {
    if !Command::new("swift").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("skipped: no swift on this machine");
        return;
    }
    let d = tmp("quartz-text");
    let script = d.join("make.swift");
    std::fs::write(&script, SWIFT_PDF).unwrap();
    let pdf = d.join("quartz.pdf");
    let o = Command::new("swift").arg(&script).arg(&pdf).output();
    let Ok(o) = o else {
        eprintln!("skipped: swift could not be started");
        return;
    };
    if !o.status.success() || !pdf.is_file() {
        eprintln!("skipped: swift could not make the PDF here: {}", String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or(""));
        return;
    }
    let theirs = squash(&String::from_utf8_lossy(&o.stdout));
    let doc = Document::open(std::fs::read(&pdf).unwrap()).unwrap();
    let ours = squash(&doc.page_text(0).unwrap());
    let want = ["The quick brown fox jumps over the lazy dog", "Monospaced line number two", "Καλημέρα κόσμε Greek in Times", "Привет мир Cyrillic in Arial", "Plain Helvetica base font line"];
    let mut missing_ours = Vec::new();
    for w in want {
        if !theirs.contains(w) {
            eprintln!("note: PDFKit did not return {w:?}; not checked for ours either");
            continue;
        }
        if !ours.contains(w) {
            missing_ours.push(w);
        }
    }
    assert!(missing_ours.is_empty(), "our reader missed {missing_ours:?}\nours:   {ours}\nPDFKit: {theirs}");
    let _ = std::fs::remove_dir_all(&d);
}
