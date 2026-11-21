use super::*;
use super::train::*;

const PARAGRAPH: [&str; 8] = [
    "The quick brown fox jumps over the lazy dog.",
    "Invoice 2024-117 was paid on 3 March: total $1,250.50.",
    "Please send the signed form to the office by Friday,",
    "and keep a copy for your own records (see page 12).",
    "Questions? Call 555-0142 or write to help@example.org.",
    "Opening hours are 9 to 5, Monday through Saturday;",
    "the library is closed on public holidays.",
    "Pack my box with five dozen liquor jugs!",
];

/// Edit distance, for the share of characters read right.
fn lev(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + (a[i - 1] != b[j - 1]) as usize);
        }
        prev = cur;
    }
    prev[b.len()]
}

fn accuracy(truth: &[&str], got: &str) -> f64 {
    let t: Vec<char> = truth.join("\n").chars().collect();
    let g: Vec<char> = got.chars().collect();
    1.0 - lev(&t, &g) as f64 / t.len() as f64
}

#[test]
fn the_shipped_table_is_well_formed() {
    let m = Model::parse(MODEL).expect("model");
    assert!(m.t.len() > 800, "{}", m.t.len());
    let chars: std::collections::BTreeSet<char> = m.t.iter().map(|t| t.ch).collect();
    for c in "abcxyzABCXYZ0189.,;:'?!@#".chars() {
        assert!(chars.contains(&c), "{c}");
    }
    assert!(Model::parse(&MODEL[..MODEL.len() - 1]).is_none() && Model::parse(b"junk").is_none() && Model::parse(&[]).is_none());
}

#[test]
fn pages_in_the_training_fonts_are_read_nearly_perfectly() {
    let fonts = training_fonts();
    if fonts.is_empty() {
        return;
    }
    for (name, f) in &fonts {
        for px in [22.0f32, 30.0, 44.0] {
            let img = page(f, &PARAGRAPH, px, 1.5, 30);
            let got = recognise(&img).text();
            let a = accuracy(&PARAGRAPH, &got);
            println!("train {name} {px}: {:.4}", a);
            assert!(a > 0.97, "{name} at {px}: {a:.3}\n{got}");
        }
    }
}

#[test]
fn pages_in_fonts_the_table_never_saw_are_read_well() {
    let fonts = heldout_fonts();
    if fonts.is_empty() {
        return;
    }
    let (mut sum, mut n) = (0.0, 0);
    for (name, f) in &fonts {
        for px in [24.0f32, 34.0, 46.0] {
            let img = page(f, &PARAGRAPH, px, 1.5, 30);
            let got = recognise(&img).text();
            let a = accuracy(&PARAGRAPH, &got);
            println!("heldout {name} {px}: {:.4}", a);
            assert!(a > 0.88, "{name} at {px}: {a:.3}\n{got}");
            sum += a;
            n += 1;
        }
    }
    let mean = sum / n as f64;
    println!("heldout mean {mean:.4}");
    assert!(mean > 0.95, "{mean}");
}

#[test]
fn word_boxes_lie_on_the_words_and_lines_come_in_reading_order() {
    let Some(f) = load(assets("DejaVuSans.ttf")) else { return };
    let img = page(&f, &["first line here", "second one"], 30.0, 1.6, 40);
    let p = recognise(&img);
    assert_eq!(p.lines.len(), 2);
    assert_eq!(p.text(), "first line here\nsecond one");
    let l = &p.lines[0];
    assert_eq!(l.words.len(), 3);
    assert!(l.words[0].x1 < l.words[1].x0 && l.words[1].x1 < l.words[2].x0);
    assert!(p.lines[0].baseline < p.lines[1].baseline);
    // the box of "first" holds dark pixels and the strip left of it does not
    let w = &l.words[0];
    let dark = (w.y0..w.y1).any(|y| (w.x0..w.x1).any(|x| img.px[y as usize * img.w + x as usize] < 100));
    assert!(dark && w.x0 >= 35);
    assert!(p.mean_confidence() > 0.5);
}

#[test]
fn a_page_with_a_little_noise_blur_and_a_slight_skew_is_still_read() {
    let Some(f) = load(assets("DejaVuSans.ttf")) else { return };
    let base = page(&f, &PARAGRAPH, 34.0, 1.5, 60);
    // salt and pepper noise from a fixed generator
    let mut rng = 0x1234_5678_9abc_def0u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut noisy = base.clone();
    for p in noisy.px.iter_mut() {
        let r = next() % 1000;
        if r < 4 {
            *p = 0;
        } else if r > 996 {
            *p = 255;
        }
    }
    let a = accuracy(&PARAGRAPH, &recognise(&noisy).text());
    println!("noise {a:.4}");
    assert!(a > 0.90, "{a}");
    // a one pixel box blur
    let mut blur = base.clone();
    for y in 1..base.h - 1 {
        for x in 1..base.w - 1 {
            let mut s = 0u32;
            for dy in 0..3 {
                for dx in 0..3 {
                    s += base.px[(y + dy - 1) * base.w + x + dx - 1] as u32;
                }
            }
            blur.px[y * base.w + x] = (s / 9) as u8;
        }
    }
    let a = accuracy(&PARAGRAPH, &recognise(&blur).text());
    println!("blur {a:.4}");
    assert!(a > 0.90, "{a}");
    // a page turned by 1.5 degrees is straightened first
    let turned = rotate(&base, 1.5);
    let p = recognise(&turned);
    let a = accuracy(&PARAGRAPH, &p.text());
    println!("skew {a:.4} (turned back by {})", p.skew);
    assert!(p.skew.abs() > 0.5 && a > 0.90, "{a} {}", p.skew);
}

#[test]
fn white_text_on_black_and_a_dim_scan_are_read() {
    let Some(f) = load(assets("DejaVuSans.ttf")) else { return };
    let base = page(&f, &PARAGRAPH[..4], 32.0, 1.5, 40);
    let inv = Gray { px: base.px.iter().map(|p| 255 - p).collect(), ..base.clone() };
    let a = accuracy(&PARAGRAPH[..4], &recognise(&inv).text());
    assert!(a > 0.95, "{a}");
    // grey paper and grey ink
    let dim = Gray { px: base.px.iter().map(|p| 90 + (*p as u32 * 110 / 255) as u8).collect(), ..base };
    let a = accuracy(&PARAGRAPH[..4], &recognise(&dim).text());
    assert!(a > 0.95, "{a}");
}

#[test]
fn blank_tiny_huge_and_hostile_pictures_give_nothing_and_never_panic() {
    assert!(recognise(&Gray { w: 100, h: 100, px: vec![255; 10_000] }).lines.is_empty());
    assert!(recognise(&Gray { w: 100, h: 100, px: vec![0; 10_000] }).lines.is_empty());
    assert!(recognise(&Gray { w: 3, h: 3, px: vec![0; 9] }).lines.is_empty());
    assert!(Gray::new(100_000, 100_000, vec![]).is_none() && Gray::new(0, 5, vec![]).is_none());
    let mut rng = 99u64;
    for size in [40usize, 97, 300] {
        let px: Vec<u8> = (0..size * size).map(|_| { rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (rng >> 56) as u8 }).collect();
        let _ = recognise(&Gray { w: size, h: size, px });
    }
    // a checkerboard (every pixel its own piece) and a grid of rules
    let px: Vec<u8> = (0..200 * 200).map(|i| if (i / 200 + i % 200) % 2 == 0 { 0 } else { 255 }).collect();
    let _ = recognise(&Gray { w: 200, h: 200, px });
    let px: Vec<u8> = (0..300 * 300).map(|i| if (i / 300) % 10 == 0 || (i % 300) % 10 == 0 { 0 } else { 255 }).collect();
    let _ = recognise(&Gray { w: 300, h: 300, px });
}

#[test]
fn look_alike_letters_are_decided_by_their_neighbours() {
    assert_eq!(repair("w0rd"), "word");
    assert_eq!(repair("HE110"), "HE110");
    assert_eq!(repair("c0de"), "code");
    assert_eq!(repair("fIat"), "flat");
    assert_eq!(repair("seCond"), "second");
    assert_eq!(repair("jumpS"), "jumps");
    assert_eq!(repair("NASA"), "NASA");
    assert_eq!(repair("McDonald"), "McDonald");
    assert_eq!(repair("CaII"), "Call");
    assert_eq!(repair_in("lnvoice", true), "Invoice");
    assert_eq!(repair_in("Iazy", false), "lazy");
    assert_eq!(repair_in("Iazy", true), "lazy");
    assert_eq!(repair_in("It", true), "It");
    assert_eq!(repair_in("WILL", false), "WILL");
    assert_eq!(repair_in("WlLL", false), "WILL");
    assert_eq!(repair("2O24"), "2024");
    assert_eq!(repair("'' x"), "\" x");
    assert_eq!(repair("100"), "100");
    assert_eq!(repair(""), "");
}

// ---------------------------------------------------------------------------------------------- scanned pages in PDF files

use crate::doc::Document;
use crate::filter::zlib_deflate;
use crate::tests_support::{assemble, stream};

/// A PDF whose one page is only a picture of `img`, scanned at `dpi`.
fn scanned_pdf(img: &Gray, dpi: f64) -> Vec<u8> {
    let (pw, ph) = (img.w as f64 * 72.0 / dpi, img.h as f64 * 72.0 / dpi);
    let data = zlib_deflate(&img.px);
    let objs = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (3, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.3} {ph:.3}] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>").into_bytes()),
        (4, stream("", format!("q {pw:.3} 0 0 {ph:.3} 0 0 cm /Im0 Do Q").as_bytes())),
        (5, stream(&format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode", img.w, img.h), &data)),
    ];
    assemble(&objs, 1, "")
}

#[test]
fn a_scan_stored_as_jbig2_is_read_as_the_same_text() {
    let Some(f) = load(assets("DejaVuSans.ttf")) else { return };
    let scan = page(&f, &PARAGRAPH, 31.0, 1.5, 60);
    // the scan made black and white and stored as one generic JBIG2 region (what a scanner's firmware writes)
    let mut bm = crate::jbig2::Bitmap::new(scan.w, scan.h, 0).unwrap();
    for (d, p) in bm.d.iter_mut().zip(&scan.px) {
        *d = (*p < 128) as u8;
    }
    let data = crate::jbig2::jbig2_tests::generic_page_stream(&bm);
    let (pw, ph) = (scan.w as f64 * 72.0 / 150.0, scan.h as f64 * 72.0 / 150.0);
    let objs = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (3, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.3} {ph:.3}] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>").into_bytes()),
        (4, stream("", format!("q {pw:.3} 0 0 {ph:.3} 0 0 cm /Im0 Do Q").as_bytes())),
        (5, stream(&format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode", scan.w, scan.h), &data)),
    ];
    let doc = Document::open(assemble(&objs, 1, "")).unwrap();
    assert!(doc.page_has_no_text(0).unwrap());
    let (text, recognised) = doc.page_text_or_ocr(0, 150.0).unwrap();
    assert!(recognised);
    let a = accuracy(&PARAGRAPH, text.trim_end());
    assert!(a > 0.95, "{a}\n{text}");
}

#[test]
fn a_scanned_pdf_page_has_no_text_of_its_own_and_its_picture_is_read() {
    let Some(f) = load(assets("DejaVuSans.ttf")) else { return };
    let scan = page(&f, &PARAGRAPH, 31.0, 1.5, 60);
    let bytes = scanned_pdf(&scan, 150.0);
    let doc = Document::open(bytes).unwrap();
    assert!(doc.page_has_no_text(0).unwrap());
    let (text, recognised) = doc.page_text_or_ocr(0, 150.0).unwrap();
    assert!(recognised);
    let a = accuracy(&PARAGRAPH, text.trim_end());
    println!("scanned pdf 150 dpi: {a:.4}");
    assert!(a > 0.97, "{a}\n{text}");
    // the same page drawn at another resolution than it was scanned at
    let (text2, _) = doc.page_text_or_ocr(0, 220.0).unwrap();
    let a2 = accuracy(&PARAGRAPH, text2.trim_end());
    println!("scanned pdf read at 220 dpi: {a2:.4}");
    assert!(a2 > 0.93, "{a2}");
}

#[test]
fn search_in_a_scan_finds_the_word_and_the_box_lies_over_it() {
    let Some(f) = load(assets("DejaVuSans.ttf")) else { return };
    let scan = page(&f, &PARAGRAPH, 31.0, 1.5, 60);
    let doc = Document::open(scanned_pdf(&scan, 150.0)).unwrap();
    let hits = doc.search_page_ocr(0, "Saturday", false, 150.0).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    // user space to the scan's pixels: 150 dpi
    let r = hits[0].rects[0];
    let (px0, px1) = (r[0] * 150.0 / 72.0, r[2] * 150.0 / 72.0);
    let page_h_pt = scan.h as f64 * 72.0 / 150.0;
    let (py0, py1) = ((page_h_pt - r[3]) * 150.0 / 72.0, (page_h_pt - r[1]) * 150.0 / 72.0);
    let ink = |x0: f64, x1: f64, y0: f64, y1: f64| {
        let mut n = 0;
        for y in (y0.max(0.0) as usize)..(y1 as usize).min(scan.h) {
            for x in (x0.max(0.0) as usize)..(x1 as usize).min(scan.w) {
                if scan.px[y * scan.w + x] < 128 {
                    n += 1;
                }
            }
        }
        n
    };
    assert!(ink(px0, px1, py0, py1) > 200, "no ink under the box: {r:?} -> {px0} {px1} {py0} {py1}");
    // and the line it is on is the sixth one: the box is lower on the page than the first line's box
    let first = doc.search_page_ocr(0, "quick", false, 150.0).unwrap();
    assert!(first[0].rects[0][1] > hits[0].rects[0][1]);
    assert!(doc.search_page_ocr(0, "zebra", false, 150.0).unwrap().is_empty());
}

#[test]
fn a_page_with_real_text_is_not_recognised_and_hostile_scans_do_not_panic() {
    let doc = Document::open(crate::tests_support::simple(&["BT /F1 12 Tf 10 50 Td (Real text) Tj ET"])).unwrap();
    let (t, recognised) = doc.page_text_or_ocr(0, 200.0).unwrap();
    assert!(!recognised && t.contains("Real text"));
    let Some(f) = load(assets("DejaVuSans.ttf")) else { return };
    let scan = page(&f, &PARAGRAPH[..3], 28.0, 1.5, 40);
    let bytes = scanned_pdf(&scan, 150.0);
    for cut in (0..bytes.len()).step_by(bytes.len() / 40 + 1) {
        if let Ok(d) = Document::open(bytes[..cut].to_vec()) {
            let _ = d.page_text_or_ocr(0, 150.0);
        }
    }
    // a picture of a kind the reader cannot draw is a grey box, not text and not a crash
    let objs = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>".to_vec()),
        (4, stream("", b"q 300 0 0 300 0 0 cm /Im0 Do Q")),
        (5, stream("/Type /XObject /Subtype /Image /Width 100 /Height 100 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /JBIG2Decode", b"\x00\x01\x02")),
    ];
    let d = Document::open(assemble(&objs, 1, "")).unwrap();
    let (t, _) = d.page_text_or_ocr(0, 150.0).unwrap();
    assert!(t.trim().is_empty());
    // a huge page is drawn at fewer dots per inch instead of being refused
    let objs = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 14000 14000] /Contents 4 0 R >>".to_vec()),
        (4, stream("", b"")),
    ];
    let d = Document::open(assemble(&objs, 1, "")).unwrap();
    assert!(d.page_ocr(0, 400.0).is_ok());
}

// ---------------------------------------------------------------------------------------------- real tools make the scans

fn have(tool: &str, arg: &str) -> bool {
    std::process::Command::new(tool).arg(arg).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn read_pgm(b: &[u8]) -> Option<Gray> {
    // P5, whitespace and comments, width, height, maxval, one whitespace, then the bytes
    let mut at = 2;
    let mut nums = vec![];
    while nums.len() < 3 {
        while at < b.len() && (b[at] as char).is_ascii_whitespace() {
            at += 1;
        }
        if b.get(at) == Some(&b'#') {
            while at < b.len() && b[at] != b'\n' {
                at += 1;
            }
            continue;
        }
        let s = at;
        while at < b.len() && b[at].is_ascii_digit() {
            at += 1;
        }
        nums.push(std::str::from_utf8(&b[s..at]).ok()?.parse::<usize>().ok()?);
    }
    if &b[..2] != b"P5" || nums[2] != 255 {
        return None;
    }
    Gray::new(nums[0], nums[1], b[at + 1..].to_vec())
}

/// A page typeset by this crate's PDF writer (Helvetica, Courier), drawn by Ghostscript (a real PDF engine with its own fonts) at
/// 150 dots per inch, then degraded by ImageMagick as a scanner would (`degrade` is its argument list). Both tools are used here to
/// MAKE test pages and are never shipped or linked. Returns the picture and the words that are on the page.
fn real_scan(tag: &str, degrade: &[&str]) -> Option<(Gray, String)> {
    if !have("gs", "--version") || !have("convert", "--version") {
        eprintln!("skipped: Ghostscript or ImageMagick is not installed");
        return None;
    }
    use crate::write::{Block, Builder, PageSize};
    let paras = [
        "The committee met on Tuesday 14 May to review the budget for next year. Revenue grew by 8.5% while costs fell, and the board approved a plan to hire twelve engineers.",
        "Please return the signed contract (two copies) to the office before noon on Friday, together with proof of address, a photograph, and the reference number 4471-B.",
        "Why does the river bend here? Nobody knows; the old maps say it has always been so. Visitors can walk the 3.2 km trail from the bridge to the mill and back.",
    ];
    let mut b = Builder::new(PageSize::A4);
    b.footer = false;
    b.blocks.push(Block::Heading(1, "Quarterly Report and Notes".into()));
    for p in paras {
        b.blocks.push(Block::Para(p.into()));
    }
    b.blocks.push(Block::Code("total = 1250.50 + 36.25\nprint(total)".into()));
    let truth = format!("Quarterly Report and Notes {} total = 1250.50 + 36.25 print(total)", paras.join(" "));
    let dir = std::env::temp_dir().join(format!("iyakki-ocr-real-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("in.pdf"), b.build().0).unwrap();
    // the pictures travel through pipes (Ghostscript in safe mode may not write where a test folder is)
    let g = std::process::Command::new("gs").args(["-q", "-dNOPAUSE", "-dBATCH", "-dFirstPage=1", "-dLastPage=1", "-sDEVICE=pgmraw", "-r150", "-dTextAlphaBits=4", "-dGraphicsAlphaBits=4", "-sOutputFile=-"]).arg(dir.join("in.pdf")).output().unwrap();
    assert!(g.status.success() && g.stdout.len() > 1000, "Ghostscript could not draw the test page: {}", String::from_utf8_lossy(&g.stderr));
    let mut c = std::process::Command::new("convert");
    c.arg("pgm:-").args(degrade).arg("pgm:-").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut si = child.stdin.take().unwrap();
    let page = g.stdout.clone();
    let w = std::thread::spawn(move || {
        use std::io::Write;
        let _ = si.write_all(&page);
    });
    let o = child.wait_with_output().unwrap();
    let _ = w.join();
    assert!(o.status.success(), "ImageMagick could not degrade the test page: {}", String::from_utf8_lossy(&o.stderr));
    let img = read_pgm(&o.stdout).expect("the degraded page is a PGM");
    let _ = std::fs::remove_dir_all(&dir);
    Some((img, truth))
}

fn words_accuracy(truth: &str, got: &str) -> f64 {
    let t: Vec<char> = truth.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    let g: Vec<char> = got.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    1.0 - lev(&t, &g) as f64 / t.len() as f64
}

#[test]
fn pages_drawn_by_ghostscript_and_degraded_by_imagemagick_are_read() {
    let cases: [(&str, &[&str], f64); 4] = [
        ("clean", &[], 0.97),
        ("noise", &["-attenuate", "0.5", "+noise", "Gaussian", "-attenuate", "0.05", "+noise", "Impulse"], 0.90),
        ("blur-skew", &["-blur", "0x0.6", "-background", "white", "-rotate", "1.2"], 0.90),
        ("low-contrast", &["-level", "35%,85%", "-blur", "0x0.5", "-attenuate", "0.2", "+noise", "Gaussian"], 0.90),
    ];
    for (tag, args, want) in cases {
        let Some((img, truth)) = real_scan(tag, args) else { return };
        let page = recognise(&img);
        let a = words_accuracy(&truth, &page.text());
        println!("real scan {tag}: {:.4} (skew found {})", a, page.skew);
        assert!(a >= want, "{tag}: {a:.3} < {want}\n{}", page.text());
    }
}

#[test]
fn a_real_scan_inside_a_pdf_is_searched_and_the_text_matches_the_source() {
    let Some((img, truth)) = real_scan("pdf", &["-attenuate", "0.2", "+noise", "Gaussian", "-blur", "0x0.5"]) else { return };
    let doc = Document::open(scanned_pdf(&img, 150.0)).unwrap();
    let (text, recognised) = doc.page_text_or_ocr(0, 150.0).unwrap();
    assert!(recognised);
    let a = words_accuracy(&truth, &text);
    println!("real scan in a PDF: {a:.4}");
    assert!(a >= 0.93, "{a}");
    // a word that was read right is found, and a word that is not on the page is not
    let mut found = 0;
    for w in ["committee", "contract", "Quarterly", "Visitors", "photograph", "engineers"] {
        if text.contains(w) {
            assert!(!doc.search_page_ocr(0, w, false, 150.0).unwrap().is_empty(), "{w} is in the recognised text but not found");
            found += 1;
        }
    }
    assert!(found >= 4, "{found} of the 6 words were read");
    assert!(doc.search_page_ocr(0, "zebra", false, 150.0).unwrap().is_empty());
}

const ACCENTED: [&str; 4] = [
    "caf\u{e9} cr\u{e8}me br\u{fb}l\u{e9}e, na\u{ef}ve fa\u{e7}ade",
    "\u{fc}ber alle K\u{e4}se und M\u{f6}hren",
    "ma\u{f1}ana ni\u{f1}o se\u{f1}or",
    "D\u{e9}j\u{e0} vu: \u{e0} la carte, \u{ea}tre ou ne pas \u{ea}tre",
];

#[test]
fn accents_are_told_from_their_letters_and_composed() {
    use crate::ocr::{compose, Accent};
    assert_eq!(compose('e', Accent::Acute), Some('\u{e9}'));
    assert_eq!(compose('n', Accent::Tilde), Some('\u{f1}'));
    assert_eq!(compose('U', Accent::Diaeresis), Some('\u{dc}'));
    assert_eq!(compose('n', Accent::Grave), None);
    assert_eq!(compose('7', Accent::Acute), None);
    let Some(f) = load(assets("DejaVuSans.ttf")) else { return };
    let (mut right, mut total) = (0, 0);
    for px in [30.0f32, 44.0] {
        let img = page(&f, &ACCENTED, px, 1.7, 30);
        let got = recognise(&img).text();
        for ch in "\u{e9}\u{e8}\u{fb}\u{ef}\u{fc}\u{e4}\u{f6}\u{f1}\u{e0}\u{ea}".chars() {
            let want = ACCENTED.iter().map(|l| l.chars().filter(|c| *c == ch).count()).sum::<usize>();
            let have = got.chars().filter(|c| *c == ch).count().min(want);
            right += have;
            total += want;
        }
    }
    // the shapes were never in the table: the accent is found by its own shape and put on the letter under it
    assert!(right * 10 >= total * 8, "{right} of {total} accented letters read\n");
}

#[test]
fn a_plain_page_still_reads_the_same_and_dots_stay_dots() {
    let Some(f) = load(assets("DejaVuSans.ttf")) else { return };
    let lines = ["this is a list: ij fill still, jj. i i i", "it is 11:30 - quiz; mix? !"];
    let img = page(&f, &lines, 34.0, 1.6, 30);
    let got = recognise(&img).text();
    assert!(!got.chars().any(|c| (c as u32) > 127), "{got}");
}

const EXTENDED: [&str; 3] = ["\u{e7}a va, fa\u{e7}ade \u{c7}a ira", "\u{10d}esk\u{fd} \u{161}kola, \u{17e}\u{e1}ba, \u{159}eka", "\u{107}ao pi\u{15b}\u{107}e \u{144} \u{15b}wiat"];

#[test]
fn the_cedilla_the_hacek_and_the_acute_of_latin_extended_are_composed_and_read() {
    use crate::ocr::{compose, Accent};
    assert_eq!(compose('c', Accent::Cedilla), Some('\u{e7}'));
    assert_eq!(compose('C', Accent::Cedilla), Some('\u{c7}'));
    assert_eq!(compose('s', Accent::Caron), Some('\u{161}'));
    assert_eq!(compose('Z', Accent::Caron), Some('\u{17d}'));
    assert_eq!(compose('e', Accent::Caron), Some('\u{11b}'));
    assert_eq!(compose('c', Accent::Acute), Some('\u{107}'));
    assert_eq!(compose('s', Accent::Cedilla), Some('\u{15f}'));
    assert_eq!(compose('e', Accent::Acute), Some('\u{e9}'), "the Latin-1 letters are as they were");
    assert_eq!(compose('a', Accent::Caron), None);
    assert_eq!(compose('c', Accent::Grave), None);
    assert_eq!(compose('m', Accent::Cedilla), None);
    let Some(f) = load(assets("DejaVuSans.ttf")) else { return };
    let (mut right, mut total) = (0, 0);
    let mut report = String::new();
    for px in [30.0f32, 44.0] {
        let img = page(&f, &EXTENDED, px, 1.7, 30);
        let got = recognise(&img).text();
        report.push_str(&got);
        report.push('\n');
        for ch in "\u{e7}\u{c7}\u{10d}\u{161}\u{17e}\u{159}\u{107}\u{15b}\u{144}".chars() {
            let want = EXTENDED.iter().map(|l| l.chars().filter(|c| *c == ch).count()).sum::<usize>();
            let have = got.chars().filter(|c| *c == ch).count().min(want);
            right += have;
            total += want;
        }
    }
    assert!(right * 10 >= total * 6, "{right} of {total} extended letters read\n{report}");
}
