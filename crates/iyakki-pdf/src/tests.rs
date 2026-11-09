use crate::tests_support::*;
use crate::*;

fn open(b: Vec<u8>) -> Document {
    Document::open(b).unwrap_or_else(|e| panic!("{e}"))
}

const HELLO: &str = "BT /F1 12 Tf 20 70 Td (Hello World) Tj 0 -20 Td (Second line) Tj ET";

#[test]
fn a_simple_file_opens_with_pages_info_and_text() {
    let d = open(simple(&[HELLO, "BT /F1 12 Tf 20 70 Td [(Page) -500 (two)] TJ ET"]));
    assert_eq!(d.page_count(), 2);
    let i = d.info();
    assert_eq!(i.title, "Test (doc)");
    assert_eq!(i.author, "Someone");
    assert_eq!(i.pages, 2);
    assert!(!i.repaired);
    assert_eq!(d.page_text(0).unwrap(), "Hello World\nSecond line");
    assert_eq!(d.page_text(1).unwrap(), "Page two");
    assert_eq!(d.page_size(0).unwrap(), (200.0, 100.0));
    assert_eq!(d.page_text(5).unwrap_err(), PdfError::PageOutOfRange);
    assert!(d.text(1000).contains("Hello World"));
}

#[test]
fn text_positioning_operators_make_lines_and_spaces() {
    let d = open(simple(&["BT /F1 10 Tf 1 0 0 1 10 80 Tm (One) Tj 30 0 Td (two) Tj ET BT /F1 10 Tf 1 0 0 1 10 65 Tm 12 TL (Three) Tj T* (Four) Tj (x) ' ET"]));
    assert_eq!(d.page_text(0).unwrap(), "One two\nThree\nFour\nx");
}

#[test]
fn search_finds_matches_with_boxes_and_context() {
    let d = open(simple(&[HELLO]));
    let hits = d.search_page(0, "world", false).unwrap();
    assert_eq!(hits.len(), 1);
    let r = hits[0].rects[0];
    // "Hello " is 6 characters at 12 pt: 20 + (667+556+222+222+556+278)/1000*12
    assert!(r[0] > 20.0 + 25.0 && r[0] < 20.0 + 40.0, "{r:?}");
    assert!(r[2] > r[0] + 30.0);
    assert!((r[1] - 70.0).abs() < 4.0);
    assert!(d.search_page(0, "world", true).unwrap().is_empty());
    assert_eq!(d.search("l", false).len(), 4);
    assert!(hits[0].context.contains("Hello World"));
}

#[test]
fn the_page_matrix_follows_rotation() {
    let mut b = simple(&[HELLO]);
    let pos = b.windows(8).position(|w| w == b"/Parent ").unwrap();
    b.splice(pos..pos, b"/Rotate 90 ".iter().copied());
    // the offsets shifted: the file is repaired by scanning, which is also the point
    let d = open(b);
    assert!(d.info().repaired);
    assert_eq!(d.page_size(0).unwrap(), (100.0, 200.0));
    let m = d.page_matrix(0, 1.0).unwrap();
    assert_eq!(m.apply(0.0, 0.0), (0.0, 0.0));
    assert_eq!(m.apply(0.0, 100.0), (100.0, 0.0));
}

#[test]
fn an_incremental_update_wins_over_the_original() {
    let orig = simple(&["BT /F1 12 Tf 20 70 Td (Old) Tj ET"]);
    let mut b = orig.clone();
    let off5 = b.len();
    b.extend_from_slice(b"5 0 obj\n");
    b.extend_from_slice(&stream("", b"BT /F1 12 Tf 20 70 Td (New) Tj ET"));
    b.extend_from_slice(b"\nendobj\n");
    let xref = b.len();
    let prev = String::from_utf8_lossy(&orig).rfind("startxref").map(|p| {
        let tail = String::from_utf8_lossy(&orig[p + 10..]).to_string();
        tail.lines().next().unwrap().trim().to_string()
    }).unwrap();
    b.extend_from_slice(format!("xref\n5 1\n{off5:010} 00000 n \ntrailer\n<< /Size 101 /Root 1 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    let d = open(b);
    assert!(!d.info().repaired);
    assert_eq!(d.page_text(0).unwrap(), "New");
    assert_eq!(d.info().title, "Test (doc)", "the trailer keys of the older section are inherited");
}

#[test]
fn xref_streams_and_object_streams_are_read() {
    let d = open(objstm_file("BT /F1 12 Tf 20 70 Td (From a stream) Tj ET"));
    assert!(!d.info().repaired);
    assert_eq!(d.page_text(0).unwrap(), "From a stream");
}

#[test]
fn a_wrong_startxref_or_a_cut_file_is_repaired() {
    let mut b = simple(&[HELLO]);
    let p = b.windows(9).rposition(|w| w == b"startxref").unwrap();
    b.truncate(p);
    b.extend_from_slice(b"startxref\n12\n%%EOF\n");
    let d = open(b);
    assert!(d.info().repaired);
    assert_eq!(d.page_text(0).unwrap(), "Hello World\nSecond line");
    // cut in the middle of the cross-reference table, so the trailer is gone too
    let b = simple(&[HELLO]);
    let p = b.windows(7).rposition(|w| w == b"\nxref\n0").unwrap() + 1;
    let d = open(b[..p + 10].to_vec());
    assert!(d.info().repaired);
    assert_eq!(d.page_text(0).unwrap(), "Hello World\nSecond line");
    // junk in front of the header
    let mut j = b"JUNK JUNK\n".to_vec();
    j.extend_from_slice(&simple(&[HELLO]));
    assert_eq!(open(j).page_text(0).unwrap(), "Hello World\nSecond line");
}

#[test]
fn non_pdf_and_encrypted_files_are_refused_with_clear_words() {
    assert_eq!(Document::open(b"hello".to_vec()).err(), Some(PdfError::NotPdf));
    assert_eq!(PdfError::NotPdf.to_string(), "this is not a PDF file");
    let b = assemble(&[(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()), (2, b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec()), (3, b"<< /Filter /Standard /V 1 /R 2 >>".to_vec())], 1, "/Encrypt 3 0 R");
    let e = Document::open(b).err().unwrap();
    assert_eq!(e, PdfError::Encrypted);
    assert_eq!(e.to_string(), "password-protected PDFs are not supported yet");
    let empty = assemble(&[(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()), (2, b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec())], 1, "");
    assert_eq!(Document::open(empty).err(), Some(PdfError::NoPages));
}

#[test]
fn cycles_in_the_page_tree_and_in_references_end() {
    let b = assemble(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [2 0 R 3 0 R 2 0 R] /Count 99999999 >>".to_vec()),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources 5 0 R >>".to_vec()),
            (4, b"4 0 R".to_vec()),
            (5, b"5 0 R".to_vec()),
        ],
        1,
        "",
    );
    let d = open(b);
    assert_eq!(d.page_count(), 1);
    assert_eq!(d.page_text(0).unwrap(), "");
    let r = d.render(0, 1.0, &BoxGlyphs).unwrap();
    assert_eq!((r.width, r.height), (612, 792));
}

#[test]
fn huge_counts_do_not_allocate_or_hang() {
    let mut b = simple(&[HELLO]);
    let s = String::from_utf8_lossy(&b).replace("0 4\n", "0 4000000000\n");
    b = s.into_bytes();
    let _ = Document::open(b);
    let c = assemble(&[(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()), (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()), (3, stream("/Type /ObjStm /N 999999999999 /First 5", b"1 0 <<>>"))], 1, "");
    let _ = Document::open(c);
}

#[test]
fn tounicode_and_type0_fonts_give_text() {
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap 1 begincodespacerange <0000> <FFFF> endcodespacerange 3 beginbfchar <0001> <0048> <0002> <0069> <0003> <00660069> endbfchar endcmap";
    let b = assemble(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>".to_vec()),
            (3, b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 6 0 R >> >> /Contents 4 0 R >>".to_vec()),
            (4, stream("", b"BT /F1 12 Tf 20 50 Td <000100020003> Tj ET")),
            (5, stream("", cmap.as_bytes())),
            (6, b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding /Identity-H /DescendantFonts [7 0 R] /ToUnicode 5 0 R >>".to_vec()),
            (7, b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Foo /DW 600 /W [1 [700 300]] >>".to_vec()),
        ],
        1,
        "",
    );
    let d = open(b);
    assert_eq!(d.page_text(0).unwrap(), "Hifi");
}

#[test]
fn encoding_differences_name_the_characters() {
    let b = assemble(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>".to_vec()),
            (3, b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec()),
            (4, stream("", b"BT /F1 12 Tf 20 50 Td (a\\001b\\002) Tj ET")),
            (5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman /Encoding << /Type /Encoding /Differences [1 /Eacute /fi] >> >>".to_vec()),
        ],
        1,
        "",
    );
    assert_eq!(open(b).page_text(0).unwrap(), "a\u{c9}b\u{fb01}");
}

#[test]
fn form_fields_are_listed_and_their_appearance_is_drawn() {
    let b = assemble(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>".to_vec()),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [5 0 R] >>".to_vec()),
            (4, stream("", b"")),
            (5, b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [10 10 60 30] /AP << /N 6 0 R >> >>".to_vec()),
            (6, stream("/Type /XObject /Subtype /Form /BBox [0 0 50 20]", b"1 0 0 rg 0 0 50 20 re f")),
        ],
        1,
        "",
    );
    let d = open(b);
    let f = d.form_fields();
    assert_eq!(f, vec![FormField { name: "name".into(), kind: "text".into(), value: "Ada".into() }]);
    assert_eq!(d.info().form_fields, 1);
    let r = d.render(0, 1.0, &BoxGlyphs).unwrap();
    let px = |x: usize, y: usize| &r.rgba[(y * r.width as usize + x) * 4..][..3];
    assert_eq!(px(30, 80), &[255, 0, 0], "the field box is red at its place (y flipped)");
    assert_eq!(px(100, 20), &[255, 255, 255]);
}

fn page_with(content: &str, extra: &str) -> Vec<u8> {
    assemble(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 40 20] >>".to_vec()),
            (3, format!("<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> {extra} >> /Contents 4 0 R >>").into_bytes()),
            (4, stream("", content.as_bytes())),
            (5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()),
        ],
        1,
        "",
    )
}

fn pixels(b: Vec<u8>) -> Rendered {
    open(b).render(0, 1.0, &BoxGlyphs).unwrap()
}

fn at(r: &Rendered, x: usize, y: usize) -> [u8; 3] {
    let i = (y * r.width as usize + x) * 4;
    [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2]]
}

#[test]
fn fills_strokes_and_clips_draw_where_they_should() {
    // page is 40 x 20; PDF y goes up
    let r = pixels(page_with("1 0 0 rg 0 0 10 10 re f 0 0 1 RG 4 w 20 15 m 36 15 l S", ""));
    assert_eq!(at(&r, 5, 15), [255, 0, 0]);
    assert_eq!(at(&r, 5, 5), [255, 255, 255]);
    assert_eq!(at(&r, 28, 5), [0, 0, 255]);
    assert_eq!(at(&r, 28, 9), [255, 255, 255]);
    // a clip to the left half, then a big green fill
    let c = pixels(page_with("0 0 20 20 re W n 0 1 0 rg 0 0 40 20 re f", ""));
    assert_eq!(at(&c, 10, 10), [0, 255, 0]);
    assert_eq!(at(&c, 30, 10), [255, 255, 255]);
    // q/Q restores the clip
    let q = pixels(page_with("q 0 0 20 20 re W n Q 0 0 1 rg 0 0 40 20 re f", ""));
    assert_eq!(at(&q, 30, 10), [0, 0, 255]);
    // alpha through an ExtGState
    let a = pixels(page_with("/G gs 0 0 0 rg 0 0 40 20 re f", "/ExtGState << /G << /ca 0.5 >> >>"));
    let v = at(&a, 10, 10)[0];
    assert!((126..=129).contains(&v), "{v}");
    // even-odd hole
    let h = pixels(page_with("0 g 0 0 40 20 re 10 5 20 10 re f*", ""));
    assert_eq!(at(&h, 20, 10), [255, 255, 255]);
    assert_eq!(at(&h, 2, 2), [0, 0, 0]);
}

#[test]
fn colour_spaces_work() {
    let r = pixels(page_with("0 1 0 0 k 0 0 10 20 re f /CS0 cs 0.5 sc 10 0 10 20 re f", "/ColorSpace << /CS0 [/Separation /Spot /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>] >>"));
    assert_eq!(at(&r, 5, 10), [255, 0, 255]);
    let g = at(&r, 15, 10)[0];
    assert!((126..=129).contains(&g), "{g}");
}

#[test]
fn images_are_drawn_scaled_and_masks_take_the_fill_colour() {
    // 2 x 2 RGB: red green / blue white, drawn over the whole 40 x 20 page
    let raw: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
    let b = assemble(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 40 20] >>".to_vec()),
            (3, b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im 5 0 R /M 6 0 R >> >> /Contents 4 0 R >>".to_vec()),
            (4, stream("", b"q 40 0 0 20 0 0 cm /Im Do Q 0 0 1 rg q 10 0 0 10 15 5 cm /M Do Q")),
            (5, stream("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8", &raw)),
            (6, stream("/Type /XObject /Subtype /Image /Width 8 /Height 1 /ImageMask true /BitsPerComponent 1", &[0b0000_1111])),
        ],
        1,
        "",
    );
    let r = pixels(b);
    assert_eq!(at(&r, 5, 3), [255, 0, 0], "top left is red");
    assert_eq!(at(&r, 35, 3), [0, 255, 0]);
    assert_eq!(at(&r, 5, 18), [0, 0, 255]);
    assert_eq!(at(&r, 35, 18), [255, 255, 255]);
    assert_eq!(at(&r, 16, 10), [0, 0, 255], "mask bits 0 paint: the left half is the fill colour");
    assert_ne!(at(&r, 23, 10), [0, 0, 255]);
}

#[test]
fn unsupported_things_are_reported_not_hidden() {
    let r = pixels(page_with("/Pattern cs /P1 scn 0 0 40 20 re f /Sh1 sh", ""));
    assert!(r.unsupported.iter().any(|u| u.contains("pattern")), "{:?}", r.unsupported);
    assert!(r.unsupported.iter().any(|u| u.contains("shading")), "{:?}", r.unsupported);
    // a JPEG 2000 picture shows a grey box and says so
    let b = assemble(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 40 20] >>".to_vec()),
            (3, b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im 5 0 R >> >> /Contents 4 0 R >>".to_vec()),
            (4, stream("", b"q 40 0 0 20 0 0 cm /Im Do Q")),
            (5, stream("/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /JPXDecode", b"xx")),
        ],
        1,
        "",
    );
    let r = pixels(b);
    assert!(r.unsupported.iter().any(|u| u.contains("JPEG 2000")), "{:?}", r.unsupported);
    assert_eq!(at(&r, 20, 10)[0], 209);
}

#[test]
fn text_is_drawn_as_boxes_without_a_font_source_at_the_right_place() {
    let r = pixels(page_with("BT /F1 10 Tf 5 5 Td (H) Tj ET", ""));
    // the box glyph is 5 x 7 pixels, its bottom on the baseline (y = 15 from the top)
    assert_eq!(at(&r, 5, 8), [0, 0, 0]);
    assert_eq!(at(&r, 7, 11), [255, 255, 255]);
}

#[test]
fn golden_hashes_of_tiny_pages_do_not_change() {
    let pages = [
        ("1 0 0 rg 2 2 20 10 re f 0 0 1 RG 3 w 0 0 m 40 20 l S", 0u64),
        ("0.5 g 20 10 m 30 18 35 2 20 2 c f 0 G [3 2] 0 d 1 w 1 19 m 39 19 l S", 0),
        ("BT /F1 8 Tf 2 8 Td (Ab) Tj ET", 0),
    ];
    let got: Vec<u64> = pages.iter().map(|(c, _)| fnv(&pixels(page_with(c, "")).rgba)).collect();
    let want = [GOLDEN[0], GOLDEN[1], GOLDEN[2]];
    assert_eq!(got, want, "if the renderer changed on purpose, put these in GOLDEN: {got:?}");
}

const GOLDEN: [u64; 3] = [15236158450560922505, 9517818217901176041, 12028290084101097893];

fn mutate(src: &[u8], rng: &mut Lcg) -> Vec<u8> {
    let mut b = src.to_vec();
    for _ in 0..1 + rng.below(6) {
        match rng.below(5) {
            0 => {
                let i = rng.below(b.len());
                b[i] = rng.next() as u8;
            }
            1 => {
                let i = rng.below(b.len());
                b.truncate(i.max(1));
            }
            2 => {
                let i = rng.below(b.len());
                let n = rng.below(20);
                b.splice(i..i, (0..n).map(|_| rng.next() as u8));
            }
            3 => {
                let i = rng.below(b.len());
                let j = (i + rng.below(40)).min(b.len());
                b.drain(i..j);
            }
            _ => {
                let i = rng.below(b.len());
                let j = rng.below(b.len());
                b[i] = b[j];
            }
        }
        if b.is_empty() {
            b.push(b'%');
        }
    }
    b
}

#[test]
fn mutated_files_never_panic_or_hang() {
    let seeds = [
        simple(&[HELLO, "0 0 1 rg 10 10 50 50 re f BT /F1 9 Tf 5 5 Td [(a) -300 (b)] TJ ET"]),
        objstm_file(HELLO),
        page_with("q 1 0 0 1 5 5 cm 0 g 0 0 m 10 0 l 10 10 l h f* Q 3 w [2] 0 d 0 0 m 30 10 40 10 40 0 c S", ""),
        assemble(
            &[
                (1, b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".to_vec()),
                (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>".to_vec()),
                (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [5 0 R] /Resources << /XObject << /X 6 0 R >> >> >>".to_vec()),
                (4, stream("", b"q 20 0 0 20 5 5 cm /X Do Q BI /W 2 /H 1 /BPC 8 /CS /G ID \x00\xff EI")),
                (5, b"<< /Subtype /Widget /FT /Tx /T (a) /V (b) /Rect [10 10 60 30] /AP << /N 6 0 R >> >>".to_vec()),
                (6, stream("/Type /XObject /Subtype /Form /BBox [0 0 50 20] /Resources << /XObject << /X 6 0 R >> >>", b"1 0 0 rg 0 0 50 20 re f /X Do")),
            ],
            1,
            "",
        ),
    ];
    let mut rng = Lcg(std::env::var("IYAKKI_PDF_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(7));
    let mut opened = 0;
    // IYAKKI_PDF_FUZZ=50000 cargo test -p iyakki-pdf mutated_files runs a longer round
    let rounds: usize = std::env::var("IYAKKI_PDF_FUZZ").ok().and_then(|v| v.parse().ok()).unwrap_or(1500);
    for round in 0..rounds {
        let seed = &seeds[round % seeds.len()];
        let m = mutate(seed, &mut rng);
        if let Ok(d) = Document::open(m) {
            opened += 1;
            let _ = d.info();
            for i in 0..d.page_count().min(3) {
                let _ = d.page_text(i);
                let _ = d.search_page(i, "a", false);
                let _ = d.render(i, 0.5, &BoxGlyphs);
            }
            let _ = d.form_fields();
        }
    }
    assert!(opened > rounds / 5, "{opened} of {rounds} mutated files still opened");
}

#[test]
fn mutated_streams_and_contents_never_panic() {
    let mut rng = Lcg(99);
    let base = b"q 1 0 0 rg 0 0 10 10 re f BT /F1 10 Tf 2 2 Td (x) Tj ET Q 5 w 0 0 m 10 10 l S BI /W 2 /H 2 /BPC 8 /CS /RGB ID aaaaaaaaaaaa EI";
    for _ in 0..800 {
        let c = mutate(base, &mut rng);
        let d = open(page_with(&String::from_utf8_lossy(&c).replace('\u{fffd}', "?"), ""));
        let _ = d.render(0, 1.0, &BoxGlyphs);
        let _ = d.page_text(0);
    }
}

#[test]
fn hostile_content_ends_quickly() {
    // thousands of q without Q, deep array nesting, a self-including form, a decompression bomb
    let mut c = "q ".repeat(50_000);
    c.push_str(&"[ ".repeat(200_000));
    c.push_str("1e9 0 0 1e9 0 0 cm 0 0 1e30 1e30 re f 0 w 1e300 1e300 m -1e300 1e-300 l S");
    let t = std::time::Instant::now();
    let d = open(page_with(&c, ""));
    let r = d.render(0, 1.0, &BoxGlyphs).unwrap();
    assert_eq!((r.width, r.height), (40, 20));
    assert!(t.elapsed().as_secs() < 20);
    let form = assemble(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 40 20] >>".to_vec()),
            (3, b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << /F 5 0 R >> >> /Contents 4 0 R >>".to_vec()),
            (4, stream("", b"/F Do")),
            (5, stream("/Subtype /Form /BBox [0 0 40 20] /Resources << /XObject << /F 5 0 R >> >>", b"0 g 0 0 5 5 re f /F Do /F Do")),
        ],
        1,
        "",
    );
    let t = std::time::Instant::now();
    let _ = open(form).render(0, 1.0, &BoxGlyphs).unwrap();
    assert!(t.elapsed().as_secs() < 10, "a form that draws itself must stop at once");
    // a page 1e9 points wide is refused, not allocated
    let big = assemble(&[(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()), (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()), (3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1000000 1000000] >>".to_vec())], 1, "");
    assert_eq!(open(big).render(0, 1.0, &BoxGlyphs).err(), Some(PdfError::TooLarge));
}

#[test]
fn a_decompression_bomb_in_a_content_stream_is_cut() {
    // fixed-Huffman block: one literal then 100 000 maximal back-references (about 25 MB out of about 160 KB in)
    let mut bits: Vec<bool> = vec![true, true, false];
    let push = |bits: &mut Vec<bool>, v: u32, n: u32| (0..n).rev().for_each(|i| bits.push(v >> i & 1 == 1));
    push(&mut bits, 0x30 + 32, 8);
    for _ in 0..100_000 {
        push(&mut bits, 0b11000101, 8);
        push(&mut bits, 0, 5);
    }
    push(&mut bits, 0, 7);
    let mut z = vec![0x78, 0x01];
    let mut bytes = vec![0u8; bits.len() / 8 + 1];
    for (i, b) in bits.iter().enumerate() {
        if *b {
            bytes[i / 8] |= 1 << (i % 8);
        }
    }
    z.extend(bytes);
    let out = crate::filter::inflate(&z, 1 << 20);
    assert!(out.len() <= 1 << 20 && out.len() > 1000);
    let b = assemble(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 40 20] >>".to_vec()),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec()),
            (4, stream("/Filter /FlateDecode", &z)),
        ],
        1,
        "",
    );
    let d = open(b);
    let _ = d.render(0, 1.0, &BoxGlyphs).unwrap();
    let _ = d.page_text(0).unwrap();
}

#[test]
fn the_writer_makes_files_the_reader_reads_back() {
    let mut b = Builder::new(PageSize::A4);
    b.title = "A \u{201c}title\u{201d} (with) brackets".into();
    b.blocks.push(Block::Heading(1, "Heading one".into()));
    b.blocks.push(Block::Para("The quick brown fox jumps over the lazy dog. ".repeat(30)));
    b.blocks.push(Block::Bullet(0, "\u{2022}".into(), "a list item with caf\u{e9}".into()));
    b.blocks.push(Block::Quote("quoted words".into()));
    b.blocks.push(Block::Code("fn main() {\n    println!(\"hi\");\n}".into()));
    b.blocks.push(Block::Rule);
    b.blocks.push(Block::Para("Tamil \u{0b95}\u{0bb2}\u{0bcd}".into()));
    let (bytes, built) = b.build();
    assert_eq!(built.replaced_chars, 3);
    assert_eq!(built.pages, 1);
    let d = open(bytes);
    assert!(!d.info().repaired);
    assert_eq!(d.info().title, "A \u{201c}title\u{201d} (with) brackets");
    let (w, h) = d.page_size(0).unwrap();
    assert!((w - 595.28).abs() < 0.01 && (h - 841.89).abs() < 0.01);
    let t = d.page_text(0).unwrap();
    assert!(t.contains("Heading one"), "{t}");
    assert!(t.contains("The quick brown fox jumps over the lazy dog. The quick"), "{t}");
    assert!(t.contains("a list item with caf\u{e9}"));
    assert!(t.contains("quoted words"));
    assert!(t.contains("println!(\"hi\");"), "{t}");
    assert!(t.contains("Tamil ???"));
    assert!(t.contains("1 / 1"), "the footer carries the page number");
    // Letter size and page breaks
    let mut l = Builder::new(PageSize::Letter);
    l.blocks = (0..120).map(|i| Block::Para(format!("Paragraph number {i}. ").repeat(8))).collect();
    let (bytes, built) = l.build();
    assert!(built.pages > 3);
    let d = open(bytes);
    assert_eq!(d.page_count(), built.pages);
    assert_eq!(d.page_size(0).unwrap(), (612.0, 792.0));
    assert!(d.text(1_000_000).contains("Paragraph number 119."));
    let n = d.search("Paragraph number 7.", true).len();
    assert!((6..=8).contains(&n), "eight copies per paragraph, one may break across a line: {n}");
}

#[test]
fn pictures_in_written_files_are_drawn_when_read_back() {
    let red: Vec<u8> = (0..40 * 40).flat_map(|_| [255, 0, 0, 255]).collect();
    let half: Vec<u8> = (0..40 * 40).flat_map(|_| [0, 0, 255, 128]).collect();
    let (a, b) = (PdfImage::from_rgba(40, 40, &red).unwrap(), PdfImage::from_rgba(40, 40, &half).unwrap());
    assert!(a.alpha.is_none() && b.alpha.is_some());
    let mut w = Builder::new(PageSize::Letter);
    w.footer = false;
    w.blocks.push(Block::Image(a));
    w.blocks.push(Block::Image(b));
    let (bytes, built) = w.build();
    assert_eq!(built.pages, 1);
    let d = open(bytes);
    let r = d.render(0, 1.0, &BoxGlyphs).unwrap();
    assert!(r.unsupported.is_empty(), "{:?}", r.unsupported);
    // 40 px at 0.75 pt each = 30 pt square at the margin (54 pt), the second one 8 pt below the first
    let px = |x: usize, y: usize| -> [u8; 3] {
        let i = (y * r.width as usize + x) * 4;
        [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2]]
    };
    assert_eq!(px(70, 54 + 15), [255, 0, 0]);
    let m = px(70, 54 + 30 + 8 + 15);
    assert!(m[0] > 120 && m[0] < 135 && m[2] > 250, "half-transparent blue over white: {m:?}");
    assert!(PdfImage::from_rgba(0, 4, &red).is_none());
}

#[test]
fn the_writer_is_safe_with_odd_text() {
    let mut b = Builder::new(PageSize::A4);
    b.blocks.push(Block::Para(String::new()));
    b.blocks.push(Block::Para("   ".into()));
    b.blocks.push(Block::Para("A".repeat(5000)));
    b.blocks.push(Block::Code("x".repeat(3000)));
    b.blocks.push(Block::Para("(unbalanced \\ ) ( parens".into()));
    b.blocks.push(Block::Heading(9, "deep".into()));
    let (bytes, _) = b.build();
    let d = open(bytes);
    assert!(d.text(100_000).contains("(unbalanced \\ ) ( parens"));
    let empty = Builder::new(PageSize::A4).build().0;
    assert_eq!(open(empty).page_count(), 1);
}

#[test]
fn page_names_parse() {
    assert_eq!(PageSize::from_name("A4"), Some(PageSize::A4));
    assert_eq!(PageSize::from_name(" letter "), Some(PageSize::Letter));
    assert_eq!(PageSize::from_name("tabloid"), None);
}

#[test]
fn a_web_page_becomes_blocks() {
    let doc = iyakki_web::doc::from_html("<html><head><title>Page title</title></head><body><h2>Section</h2><p>Some <b>bold</b> text.</p><ul><li>one</li><li>two</li></ul><pre>code  here</pre><table><tr><td>a</td><td>bb</td></tr><tr><td>ccc</td><td>d</td></tr></table></body></html>");
    let blocks = crate::docconv::blocks_from_doc(&doc, &|_| None);
    assert_eq!(blocks[0], Block::Heading(1, "Page title".into()));
    assert!(blocks.contains(&Block::Heading(2, "Section".into())));
    assert!(blocks.contains(&Block::Para("Some bold text.".into())));
    assert!(blocks.iter().any(|b| matches!(b, Block::Bullet(_, _, t) if t == "two")));
    assert!(blocks.iter().any(|b| matches!(b, Block::Code(c) if c.contains("code  here"))));
    assert!(blocks.iter().any(|b| matches!(b, Block::Code(c) if c.contains("ccc") && c.contains("bb"))));
}
