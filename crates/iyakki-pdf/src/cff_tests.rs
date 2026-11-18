//! CFF fonts without a Unicode map (F-PDF-18, KI-960): a PDF whose simple font carries a Type1C program with its own encoding is read
//! through the program's glyph names; a program the standard names do not cover stays unread (and says nothing wrong).

use crate::cff::build;
use crate::tests_support::{assemble, stream};
use crate::Document;

fn pdf_with(program: &[u8], font_extra: &str, shown: &[u8]) -> Vec<u8> {
    let mut content = b"BT /F1 12 Tf 10 50 Td (".to_vec();
    for &b in shown {
        content.extend_from_slice(format!("\\{:03o}", b).as_bytes());
    }
    content.extend_from_slice(b") Tj ET");
    let objs = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>".to_vec()),
        (3, b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec()),
        (4, stream("", &content)),
        (5, format!("<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Private /FontDescriptor 6 0 R {font_extra} >>").into_bytes()),
        (6, b"<< /Type /FontDescriptor /FontName /ABCDEF+Private /Flags 4 /FontFile3 7 0 R >>".to_vec()),
        (7, stream("/Subtype /Type1C", program)),
    ];
    assemble(&objs, 1, "")
}

#[test]
fn a_cff_font_with_a_private_code_order_is_read_through_its_own_glyph_names() {
    let prog = build::font(&["Omega", "eacute", "alpha", "uni0416", "g17"], &[1, 2, 3, 4, 5], &[]);
    let d = Document::open(pdf_with(&prog, "", &[1, 2, 3, 4, 5])).unwrap();
    // g17 is a name nothing explains: no character, not a wrong one
    assert_eq!(d.page_text(0).unwrap(), "ΩéαЖ");
}

#[test]
fn the_pdf_encoding_still_wins_when_it_names_a_base_or_differences() {
    let prog = build::font(&["Omega", "eacute"], &[1, 2], &[]);
    // differences on top of the program's own encoding: code 2 is changed to /beta, code 1 stays the program's
    let d = Document::open(pdf_with(&prog, "/Encoding << /Type /Encoding /Differences [2 /beta] >>", &[1, 2])).unwrap();
    assert_eq!(d.page_text(0).unwrap(), "Ωβ");
    // an explicit base encoding is the PDF's word: WinAnsi code 1 has no character
    let d = Document::open(pdf_with(&prog, "/Encoding /WinAnsiEncoding", &[65, 66])).unwrap();
    assert_eq!(d.page_text(0).unwrap(), "AB");
}

#[test]
fn a_damaged_program_leaves_the_font_as_before() {
    let d = Document::open(pdf_with(b"\x01\x00\x04\x02garbage", "", &[65, 66])).unwrap();
    assert_eq!(d.page_text(0).unwrap(), "AB");
}

#[test]
fn a_font_with_a_private_encoding_written_by_ghostscript_reads_as_pdftotext_reads_it() {
    use std::process::Command;
    let have = |t: &str, a: &str| Command::new(t).arg(a).output().is_ok();
    if !have("gs", "--version") || !have("pdftotext", "-v") {
        eprintln!("skipped: gs or pdftotext is missing");
        return;
    }
    let d = std::path::PathBuf::from("/tmp").join(format!("iyakki-cff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    // a font whose codes 1..9 are H e l o w r d é and a space, in an order no standard encoding has
    let ps = "%!PS\n/Times-Roman findfont dup length dict begin\n{ 1 index /FID ne {def} {pop pop} ifelse } forall\n/Encoding 256 array 0 1 255 {1 index exch /.notdef put} for\n\
dup 1 /H put dup 2 /e put dup 3 /l put dup 4 /o put dup 5 /w put dup 6 /r put dup 7 /d put dup 8 /eacute put dup 9 /space put\ndef\ncurrentdict end /Priv exch definefont pop\n\
/Priv findfont 14 scalefont setfont\n50 700 moveto <010203030409050407060307> show\n50 680 moveto <0108030309> show 50 660 moveto <04070607> show\nshowpage\n";
    std::fs::write(d.join("a.ps"), ps).unwrap();
    let o = Command::new("gs").env("TMPDIR", "/tmp").args(["-q", "-dNOPAUSE", "-dBATCH", "-dSAFER", "-sDEVICE=pdfwrite", "-dCompatibilityLevel=1.7"]).arg(format!("-sOutputFile={}", d.join("a.pdf").display())).arg(d.join("a.ps")).output().unwrap();
    assert!(o.status.success());
    let bytes = std::fs::read(d.join("a.pdf")).unwrap();
    assert!(bytes.windows(7).any(|w| w == b"Type1C\n" || w == b"/Type1C") , "ghostscript wrote a CFF program");
    let theirs = Command::new("pdftotext").arg(d.join("a.pdf")).arg("-").output().unwrap();
    let theirs: String = String::from_utf8_lossy(&theirs.stdout).split_whitespace().collect::<Vec<_>>().join(" ");
    let doc = Document::open(bytes).unwrap();
    let ours: String = doc.page_text(0).unwrap().split_whitespace().collect::<Vec<_>>().join(" ");
    let _ = std::fs::remove_dir_all(&d);
    assert!(theirs.contains("Hello wodrld"), "{theirs}");
    assert_eq!(ours, theirs);
}
