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

// ------------------------------------------------------------------------------- CID fonts and OpenType-wrapped programs

/// A page that shows the two-byte codes `shown` with a Type 0 font (Identity-H) over a CIDFontType0 whose program is `program`.
fn cid_pdf(program: &[u8], subtype: &str, to_unicode: Option<&str>, shown: &[u16]) -> Vec<u8> {
    let hex: String = shown.iter().map(|c| format!("{c:04X}")).collect();
    let content = format!("BT /F1 12 Tf 10 50 Td <{hex}> Tj ET").into_bytes();
    let tu = if to_unicode.is_some() { "/ToUnicode 9 0 R" } else { "" };
    let mut objs = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>".to_vec()),
        (3, b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec()),
        (4, stream("", &content)),
        (5, format!("<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+Cid /Encoding /Identity-H /DescendantFonts [8 0 R] {tu} >>").into_bytes()),
        (6, b"<< /Type /FontDescriptor /FontName /ABCDEF+Cid /Flags 4 /FontFile3 7 0 R >>".to_vec()),
        (7, stream(&format!("/Subtype /{subtype}"), program)),
        (8, b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /ABCDEF+Cid /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 6 0 R /DW 1000 >>".to_vec()),
    ];
    if let Some(t) = to_unicode {
        objs.push((9, stream("", t.as_bytes())));
    }
    assemble(&objs, 1, "")
}

#[test]
fn a_cid_font_whose_program_is_not_cid_keyed_is_read_through_the_glyph_names_with_the_cid_as_glyph_number() {
    let prog = build::font(&["Omega", "eacute", "alpha", "g17"], &[1, 2, 3, 4], &[]);
    let d = Document::open(cid_pdf(&prog, "CIDFontType0C", None, &[1, 2, 3, 3, 1])).unwrap();
    assert_eq!(d.page_text(0).unwrap(), "ΩéααΩ");
}

#[test]
fn an_opentype_wrapped_program_is_read_through_its_character_map_and_names() {
    let prog = build::font(&["g1", "g2", "g3"], &[1, 2, 3], &[]);
    // the names mean nothing; the OpenType file's character map says what glyphs 1 to 3 are
    let wrapped = build::otto(&prog, &[(0x416, 1), (0x3b1, 2), (0x65e5, 3)]);
    let d = Document::open(cid_pdf(&wrapped, "OpenType", None, &[3, 1, 2])).unwrap();
    assert_eq!(d.page_text(0).unwrap(), "日Жα");
}

#[test]
fn a_cid_keyed_opentype_program_maps_each_cid_to_its_glyph_and_then_to_the_character() {
    // CIDs 100, 200, 300 sit in glyphs 1, 2, 3: the page shows CIDs, the cmap knows glyphs
    let prog = build::cid_font(&[100, 200, 300], ("Adobe", "Identity", 0));
    let wrapped = build::otto(&prog, &[(0x41, 1), (0x42, 2), (0x3042, 3)]);
    let d = Document::open(cid_pdf(&wrapped, "OpenType", None, &[300, 100, 200])).unwrap();
    assert_eq!(d.page_text(0).unwrap(), "あAB");
}

#[test]
fn the_fonts_own_to_unicode_map_still_wins_and_a_bare_cid_keyed_program_stays_unread_without_one() {
    let prog = build::cid_font(&[100, 200], ("Adobe", "Identity", 0));
    let map = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n1 begincodespacerange <0000> <FFFF> endcodespacerange\n1 beginbfchar <0064> <0058> endbfchar\nendcmap end end";
    let with = Document::open(cid_pdf(&prog, "CIDFontType0C", Some(map), &[100])).unwrap();
    assert_eq!(with.page_text(0).unwrap(), "X");
    // no ToUnicode and no names or character map anywhere: nothing is invented
    let without = Document::open(cid_pdf(&prog, "CIDFontType0C", None, &[100, 200])).unwrap();
    assert_eq!(without.page_text(0).unwrap(), "");
}

#[test]
fn an_opentype_program_in_a_simple_font_gives_its_private_encoding() {
    let prog = build::font(&["Omega", "eacute", "alpha"], &[1, 2, 3], &[]);
    let wrapped = build::otto(&prog, &[(0x3a9, 1)]);
    let d = Document::open(pdf_with(&wrapped, "", &[1, 2, 3])).unwrap();
    assert_eq!(d.page_text(0).unwrap(), "Ωéα");
}

#[test]
fn damaged_cid_programs_in_a_page_never_panic() {
    let prog = build::otto(&build::cid_font(&[100, 200, 300], ("Adobe", "Identity", 0)), &[(0x41, 1), (0x42, 2)]);
    let mut seed = 7u32;
    for _ in 0..200 {
        let mut g = prog.clone();
        for _ in 0..3 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let i = (seed >> 8) as usize % g.len();
            g[i] = (seed >> 24) as u8;
        }
        if let Ok(d) = Document::open(cid_pdf(&g, "OpenType", None, &[100, 200, 300])) {
            let _ = d.page_text(0);
        }
    }
}

/// The first font of the Noto Sans CJK collection as a stand-alone OpenType file with only the tables the reader needs (a real,
/// CID-keyed CFF program). `None` where the collection is not installed.
fn real_cid_keyed_font() -> Option<Vec<u8>> {
    let ttc = std::fs::read("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc").ok()?;
    let u32_at = |o: usize| u32::from_be_bytes(ttc[o..o + 4].try_into().unwrap()) as usize;
    if &ttc[..4] != b"ttcf" {
        return None;
    }
    let font = u32_at(12);
    let n = u16::from_be_bytes([ttc[font + 4], ttc[font + 5]]) as usize;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for i in 0..n {
        let e = font + 12 + 16 * i;
        let tag: [u8; 4] = ttc[e..e + 4].try_into().unwrap();
        if [*b"CFF ", *b"cmap", *b"maxp"].contains(&tag) {
            let (off, len) = (u32_at(e + 8), u32_at(e + 12));
            tables.push((tag, ttc[off..off + len].to_vec()));
        }
    }
    Some(build::sfnt(b"OTTO", &tables))
}

#[test]
fn a_real_cid_keyed_opentype_font_is_read_through_its_charset_and_character_map() {
    let Some(font) = real_cid_keyed_font() else {
        eprintln!("skipped: Noto Sans CJK is not installed");
        return;
    };
    let cff = crate::cff::Cff::parse(&font).expect("the program inside the file is read");
    assert!(cff.cid_keyed && cff.cids.len() > 10_000, "{} glyphs", cff.cids.len());
    let ros = cff.ros.clone().expect("a CID-keyed program names its registry and ordering");
    assert_eq!(ros.0, "Adobe");
    // the CIDs of three characters, found through the file's own character map and charset
    let ttf = crate::ttf::Ttf::cmap_only(&font).unwrap();
    let map = ttf.cmap_map();
    let want = ['日', '本', 'A', 'あ', 'α'];
    let cids: Vec<u16> = want.iter().map(|c| cff.cids[map[&(*c as u32)] as usize]).collect();
    let d = Document::open(cid_pdf(&font, "OpenType", None, &cids)).unwrap();
    let ours: String = d.page_text(0).unwrap().split_whitespace().collect();
    assert_eq!(ours, want.iter().collect::<String>());
    // another reader: poppler takes the file as an embedded CID font with OpenType outlines (it has no way to turn an Identity code
    // into a character without a ToUnicode map, so only the font reading is compared)
    let have = |t: &str, a: &str| std::process::Command::new(t).arg(a).output().is_ok();
    if have("pdffonts", "-v") {
        let dir = std::path::PathBuf::from("/tmp").join(format!("iyakki-cidcff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.pdf"), cid_pdf(&font, "OpenType", None, &cids)).unwrap();
        let o = std::process::Command::new("pdffonts").arg(dir.join("a.pdf")).output().unwrap();
        let listing = String::from_utf8_lossy(&o.stdout).to_string();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(listing.contains("CID Type 0C (OT)") && listing.contains("yes"), "{listing}");
    }
}
