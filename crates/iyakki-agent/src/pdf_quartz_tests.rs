//! The PDF renderer against a second renderer: the same one-page drawings in macOS's own PDF engine (`sips`, Core Graphics) (F-BROW-35). Each page holds only
//! shapes (no text: the fonts differ), is rendered by `iyakki_pdf` at 72 dpi and by `sips`, both laid over white, and compared pixel by pixel. Skipped, with a
//! printed line, where there is no `sips` (that is, everywhere but macOS).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use iyakki_pdf::doc::Document;
use iyakki_pdf::render::BoxGlyphs;

const W: usize = 200;
const H: usize = 100;

fn assemble(content: &str) -> Vec<u8> {
    let objects: Vec<(u32, String)> = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".into()),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >>".into()),
        (3, "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".into()),
        (4, format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offs = vec![0usize; 5];
    for (n, body) in &objects {
        offs[*n as usize] = out.len();
        out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for n in 1..=4 {
        out.extend_from_slice(format!("{:010} 00000 n \n", offs[n]).as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

fn have_sips() -> bool {
    Command::new("sips").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-pdfq-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn quartz(dir: &Path, name: &str, pdf: &[u8]) -> Result<Vec<u8>, String> {
    let (i, o) = (dir.join(format!("{name}.pdf")), dir.join(format!("{name}.png")));
    std::fs::write(&i, pdf).unwrap();
    let st = Command::new("sips").args(["-s", "format", "png"]).arg(&i).arg("--out").arg(&o).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("sips failed".into());
    }
    let png = std::fs::read(&o).map_err(|e| e.to_string())?;
    let b = crate::qaimage::decode_png(&png)?;
    if (b.w as usize, b.h as usize) != (W, H) {
        return Err(format!("sips made a {}x{} picture", b.w, b.h));
    }
    Ok(over_white(&b.rgba))
}

fn over_white(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            let m = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
            [m(p[0]), m(p[1]), m(p[2]), 255]
        })
        .collect()
}

fn ours(pdf: &[u8]) -> Vec<u8> {
    let d = Document::open(pdf.to_vec()).expect("our reader opens it");
    let r = d.render(0, 1.0, &BoxGlyphs).expect("our renderer draws it");
    assert_eq!((r.width as usize, r.height as usize), (W, H));
    over_white(&r.rgba)
}

/// Core Graphics converts colours through its colour profiles (a pure red comes out as 234, 51, 35), so colours are not compared: what is compared is
/// which pixels carry ink at all (a channel more than 60 below white), the share of pixels where the two pictures disagree about that.
fn diff(a: &[u8], b: &[u8]) -> (f32, f32) {
    let ink = |v: &[u8], i: usize| (0..3).any(|c| 255 - v[i * 4 + c] as i32 > 60);
    let n = a.len() / 4;
    let bad = (0..n).filter(|&i| ink(a, i) != ink(b, i)).count();
    let total = (0..n).filter(|&i| ink(b, i)).count().max(1);
    (bad as f32 * 100.0 / n as f32, bad as f32 * 100.0 / total as f32)
}

fn drawings() -> Vec<(&'static str, &'static str, f32)> {
    vec![
        ("filled rectangles", "1 0 0 rg 20 20 60 40 re f 0 0.6 0 rg 100 30 50 50 re f 0 0 1 rg 60 50 40 30 re f", 1.5),
        ("strokes and widths", "0 0 0 RG 6 w 20 80 m 180 80 l S 1 w 20 70 m 180 70 l S 12 w 1 J 30 40 m 170 40 l S 0 J 3 w [8 4] 0 d 20 20 m 180 20 l S", 2.0),
        ("curves and closed paths", "0.8 0 0.8 rg 20 20 m 20 80 100 80 100 20 c f 0 0.4 0.8 RG 4 w 120 20 m 140 90 180 90 180 20 c 120 20 l S", 2.0),
        ("even odd and nonzero", "0.9 0.5 0 rg 10 10 80 80 re 30 30 40 40 re f* 0.2 0.5 0.9 rg 110 10 80 80 re 130 30 40 40 re f", 1.5),
        ("transforms and q Q", "q 0.7071 0.7071 -0.7071 0.7071 100 10 cm 0.2 0.6 0.2 rg 0 0 50 50 re f Q q 2 0 0 0.5 10 60 cm 0.8 0.2 0.2 rg 0 0 40 40 re f Q 0 0 0 rg 150 10 30 30 re f", 2.5),
        ("clipping", "q 100 50 m 170 50 l 135 95 l h W n 1 0.4 0 rg 90 40 100 60 re f Q q 20 20 60 60 re W n 0 0.5 1 rg 0 0 200 100 re f 1 1 1 rg 30 30 20 20 re f Q", 2.5),
        ("fill and stroke together", "0.9 0.9 0.2 rg 0 0 0 RG 5 w 20 20 70 60 re B 1 j 110 20 m 180 20 l 145 80 l b* 0.4 g 2 w 100 10 m 100 90 l S", 2.0),
        ("v curve only", "0 0 0 RG 3 w 20 20 m 60 90 100 20 v S", 1.0),
        ("y curve only", "0 0 0 RG 3 w 110 20 m 110 90 150 80 y S", 1.0),
        ("circle of four curves", "0 0 0 RG 3 w 160 50 m 160 70 180 70 180 50 c 180 30 160 30 160 50 c S", 1.0),
        ("v and y curves and arcs", "0 0 0 RG 3 w 20 20 m 60 90 100 20 v S 110 20 m 110 90 150 80 y S 160 50 m 160 70 180 70 180 50 c 180 30 160 30 160 50 c S", 2.5),
        ("dashes miter and nested transforms", "0 0 0 RG 4 w [10 5 2 5] 3 d 20 90 m 180 90 l S [] 0 d 8 w 0 j 10 M 20 20 m 60 60 l 100 20 l S q 0.5 0 0 0.5 120 10 cm q 2 0 0 2 0 0 cm 0 0 40 40 re S Q 0 0 0 rg 60 0 30 30 re f Q", 2.5),
        ("grey cmyk and join styles", "0.5 g 10 10 40 80 re f 0.3 0.7 0.1 0 k 60 10 40 80 re f 0 0 0 1 RG 10 w 0 j 120 20 m 150 80 l 180 20 l S 1 j 120 20 m 150 60 l 180 20 l S", 2.5),
    ]
}

#[test]
fn the_pdf_renderer_agrees_with_the_core_graphics_of_macos_on_shape_only_pages() {
    if !have_sips() {
        eprintln!("no sips here: this test did not run");
        return;
    }
    let dir = tmp();
    let all = drawings();
    let mut bad = Vec::new();
    for (i, (name, content, allowed)) in all.iter().enumerate() {
        let pdf = assemble(content);
        let (a, b) = (ours(&pdf), quartz(&dir, &format!("d{i}"), &pdf).unwrap_or_else(|e| panic!("{name}: {e}")));
        let (share, mean) = diff(&a, &b);
        eprintln!("pdf vs Core Graphics: {name}: {share:.2} % of the pixels disagree on ink ({mean:.2} % of the reference ink)");
        let ink = b.chunks(4).filter(|p| p[0] < 200 || p[1] < 200 || p[2] < 200).count();
        assert!(ink > 150, "{name}: the reference drew almost nothing");
        let other = ours(&assemble(all[(i + 1) % all.len()].1));
        assert!(diff(&other, &b).0 > 2.0 * allowed, "{name}: another drawing is not told apart from it");
        if share > *allowed {
            bad.push(format!("{name}: {share:.2} % differ (allowed {allowed}), {mean:.2} % of the ink"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    let _ = std::fs::remove_dir_all(dir);
}
