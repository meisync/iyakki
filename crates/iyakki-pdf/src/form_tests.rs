use crate::tests_support::{assemble, stream, Lcg};
use crate::{BoxGlyphs, Document};

fn form_pdf() -> Vec<u8> {
    assemble(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R 8 0 R 11 0 R] /DA (/Helv 0 Tf 0 g) >> >>".to_vec()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>".to_vec()),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R 11 0 R] >>".to_vec()),
            (4, stream("", b"")),
            (5, b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [10 70 110 90] /DA (/Helv 12 Tf 0 0 1 rg) /MaxLen 20 /AP << /N 9 0 R >> >>".to_vec()),
            (6, b"<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [10 40 25 55] /AP << /N << /Yes 10 0 R /Off 9 0 R >> >> >>".to_vec()),
            (7, b"<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131072 /T (colour) /V (red) /Opt [(red) (green) [(b) (blue)]] /Rect [40 40 110 55] >>".to_vec()),
            (8, b"<< /Type /Annot /Subtype /Widget /FT /Tx /Ff 1 /T (locked) /V (fixed) /Rect [120 70 190 90] >>".to_vec()),
            (9, stream("/Type /XObject /Subtype /Form /BBox [0 0 100 20]", b"1 0 0 rg 0 0 100 20 re f")),
            (10, stream("/Type /XObject /Subtype /Form /BBox [0 0 15 15]", b"0 1 0 rg 0 0 15 15 re f")),
            (11, b"<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 32768 /T (pick) /V /A /Rect [120 40 135 55] >>".to_vec()),
        ],
        1,
        "",
    )
}

/// The same form in a file whose cross-reference is a stream (so the update must be one too).
fn form_pdf_xref_stream() -> Vec<u8> {
    let bodies: [(u32, Vec<u8>); 6] = [
        (1, b"<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>".to_vec()),
        (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [5 0 R] >>".to_vec()),
        (4, stream("", b"")),
        (5, b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [10 70 110 90] >>".to_vec()),
        (6, b"<< /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) >>".to_vec()),
    ];
    let mut out = b"%PDF-1.5\n".to_vec();
    let mut offs = Vec::new();
    for (n, b) in &bodies {
        offs.push(out.len());
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(b);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xo = out.len();
    let mut x: Vec<u8> = vec![0, 0, 0, 0, 0, 0, 255];
    for o in offs.iter().chain(std::iter::once(&xo)) {
        x.push(1);
        x.extend_from_slice(&(*o as u32).to_be_bytes());
        x.extend_from_slice(&0u16.to_be_bytes());
    }
    out.extend_from_slice(b"7 0 obj\n");
    out.extend_from_slice(&stream("/Type /XRef /Size 8 /W [1 4 2] /Root 1 0 R", &x));
    out.extend_from_slice(format!("\nendobj\nstartxref\n{xo}\n%%EOF\n").as_bytes());
    out
}

fn open(b: Vec<u8>) -> Document {
    Document::open(b).unwrap()
}

fn ch(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

fn value_of(d: &Document, name: &str) -> String {
    d.editable_fields().into_iter().find(|f| f.name == name).map(|f| f.value).unwrap_or_default()
}

#[test]
fn the_fields_are_told_apart_by_kind_and_what_they_allow() {
    use crate::form::FieldKind;
    let d = open(form_pdf());
    let f = d.editable_fields();
    let by = |n: &str| f.iter().find(|x| x.name == n).unwrap().clone();
    assert!(matches!(by("name").kind, FieldKind::Text { multiline: false, password: false, max_len: Some(20) }));
    assert_eq!(by("agree").kind, FieldKind::Check { on: "Yes".into() });
    assert_eq!(by("colour").kind, FieldKind::Choice { options: vec!["red".into(), "green".into(), "blue".into()], edit: false });
    assert!(by("locked").read_only && !by("locked").editable());
    assert_eq!(by("pick").kind, FieldKind::Radio);
    assert!(by("name").editable() && by("agree").editable());
    assert_eq!(by("name").value, "Ada");
}

#[test]
fn filling_writes_an_incremental_update_the_original_stays_and_the_values_read_back() {
    let orig = form_pdf();
    let d = open(orig.clone());
    let out = d.fill_form(&ch(&[("name", "Grace Hopper"), ("agree", "true"), ("colour", "green")])).unwrap();
    assert!(out.starts_with(&orig), "every original byte is untouched");
    assert!(out.len() > orig.len());
    assert_eq!(out.windows(9).filter(|w| *w == b"startxref").count(), 2, "a second cross-reference section was appended");
    assert!(String::from_utf8_lossy(&out[orig.len()..]).contains("/Prev "));
    let n = open(out.clone());
    assert_eq!(value_of(&n, "name"), "Grace Hopper");
    assert_eq!(value_of(&n, "colour"), "green");
    assert_eq!(value_of(&n, "agree"), "Yes");
    assert_eq!(value_of(&n, "locked"), "fixed");
    assert_eq!(n.page_count(), 1);
    // the appearance is new: the red box is gone where the text is drawn, and the check box shows its checked picture (green)
    let (b, a) = (d.render(0, 1.0, &BoxGlyphs).unwrap(), n.render(0, 1.0, &BoxGlyphs).unwrap());
    let px = |r: &crate::Rendered, x: usize, y: usize| r.rgba[(y * r.width as usize + x) * 4..][..3].to_vec();
    assert_eq!(px(&b, 105, 20), vec![255, 0, 0], "before: the stored red appearance");
    assert_ne!(px(&a, 105, 20), vec![255, 0, 0], "after: the new appearance replaced it");
    assert_eq!(px(&b, 17, 52), vec![255, 0, 0], "before: the unchecked appearance (red in this file)");
    assert_eq!(px(&a, 17, 52), vec![0, 255, 0], "after: the checked appearance is drawn");
    // a second fill on the result chains
    let again = n.fill_form(&ch(&[("name", "Ada Lovelace")])).unwrap();
    assert!(again.starts_with(&out));
    let n2 = open(again);
    assert_eq!((value_of(&n2, "name").as_str(), value_of(&n2, "agree").as_str(), value_of(&n2, "colour").as_str()), ("Ada Lovelace", "Yes", "green"));
    // and a check box can be cleared
    let off = n2.fill_form(&ch(&[("agree", "no")])).unwrap();
    assert_eq!(value_of(&open(off), "agree"), "Off");
}

#[test]
fn a_file_with_a_cross_reference_stream_gets_a_stream_update() {
    let orig = form_pdf_xref_stream();
    let d = open(orig.clone());
    assert_eq!(value_of(&d, "name"), "Ada");
    let out = d.fill_form(&ch(&[("name", "Émile (the \\ writer)")])).unwrap();
    assert!(out.starts_with(&orig));
    let tail = String::from_utf8_lossy(&out[orig.len()..]).to_string();
    assert!(tail.contains("/Type /XRef") && !tail.contains("\nxref\n"), "{tail}");
    let n = open(out);
    assert_eq!(value_of(&n, "name"), "Émile (the \\ writer)");
    assert!(!n.file.repaired, "the new index was read, not rebuilt by scanning");
}

#[test]
fn what_cannot_be_filled_is_refused_with_the_reason_and_nothing_is_written() {
    let d = open(form_pdf());
    let err = |c: &[(&str, &str)]| d.fill_form(&ch(c)).unwrap_err();
    assert!(err(&[("nothing", "x")]).contains("no field named"));
    assert!(err(&[("locked", "x")]).contains("read-only"));
    assert!(err(&[("colour", "purple")]).contains("not one of the choices"));
    assert!(err(&[("name", "this text is far too long")]).contains("at most 20"));
    assert!(err(&[("name", "two\nlines")]).contains("one line"));
    assert!(err(&[("pick", "x")]).contains("radio"));
    assert!(err(&[]).contains("nothing to change"));
    // a name in another case is found when it is unique
    assert!(d.fill_form(&ch(&[("NAME", "Bo")])).is_ok());
}

#[test]
fn protected_and_damaged_files_are_not_updated() {
    for name in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data")).unwrap().flatten() {
        let n = name.file_name().to_string_lossy().to_string();
        if n.contains("rc4") || n.contains("aes") {
            if let Ok(d) = Document::open_with_password(std::fs::read(name.path()).unwrap(), "open-sesame") {
                assert!(d.fill_form(&ch(&[("a", "b")])).is_err(), "{n}");
            }
        }
    }
    let mut broken = form_pdf();
    let at = broken.windows(9).rposition(|w| w == b"startxref").unwrap();
    broken.truncate(at);
    if let Ok(d) = Document::open(broken) {
        assert!(d.fill_form(&ch(&[("name", "x")])).is_err());
    }
}

#[test]
fn text_is_escaped_aligned_and_sized_by_the_fields_own_settings() {
    let s = |v: &str, w: f64, h: f64, da: Option<&str>, q: i64, m: bool, p: bool| String::from_utf8_lossy(&crate::form::appearance_stream(v, w, h, da, q, m, p)).to_string();
    let a = s("a (b) \\ c", 100.0, 20.0, Some("/Helv 10 Tf 1 0 0 rg"), 0, false, false);
    assert!(a.contains("a \\(b\\) \\\\ c") && a.contains("1 0 0 rg") && a.contains("/Helv 10 Tf"), "{a}");
    let left = s("x", 100.0, 20.0, None, 0, false, false);
    let right = s("x", 100.0, 20.0, None, 2, false, false);
    assert!(left.contains("1 0 0 1 2 ") && right.contains("1 0 0 1 9") , "{left} / {right}");
    let pw = s("secret", 100.0, 20.0, None, 0, false, true);
    assert!(pw.contains("(******)") && !pw.contains("secret"));
    let ml = s("one\ntwo", 100.0, 60.0, Some("/Helv 0 Tf 0 g"), 0, true, false);
    assert!(ml.contains("(one)") && ml.contains("(two)") && ml.contains("/Helv 12 Tf"), "{ml}");
    let uni = s("日本", 100.0, 20.0, None, 0, false, false);
    assert!(uni.contains("(??)"));
}

#[test]
fn text_outside_latin_is_stored_whole_in_the_value() {
    let d = open(form_pdf());
    let out = d.fill_form(&ch(&[("name", "日本語 ok")])).unwrap();
    assert_eq!(value_of(&open(out), "name"), "日本語 ok");
}

#[test]
fn hostile_files_never_panic_when_filled() {
    let base = form_pdf();
    let mut r = Lcg(7);
    for round in 0..300 {
        let mut b = base.clone();
        for _ in 0..1 + round % 6 {
            let i = r.below(b.len());
            b[i] = r.next() as u8;
        }
        if let Ok(d) = Document::open(b) {
            let _ = d.editable_fields();
            let _ = d.fill_form(&ch(&[("name", "x"), ("agree", "on"), ("colour", "red")]));
        }
    }
}

/// For checking with another reader by hand: `IYAKKI_FORM_OUT=dir cargo test -p iyakki-pdf --lib write_filled_samples -- --ignored`.
#[test]
#[ignore = "writes files for a manual check with another PDF reader"]
fn write_filled_samples_for_other_readers() {
    let dir = std::env::var("IYAKKI_FORM_OUT").expect("set IYAKKI_FORM_OUT to a folder");
    let a = open(form_pdf()).fill_form(&ch(&[("name", "Grace Hopper"), ("agree", "true"), ("colour", "green")])).unwrap();
    std::fs::write(format!("{dir}/classic-filled.pdf"), a).unwrap();
    let b = open(form_pdf_xref_stream()).fill_form(&ch(&[("name", "Ada Lovelace")])).unwrap();
    std::fs::write(format!("{dir}/xrefstream-filled.pdf"), b).unwrap();
}
