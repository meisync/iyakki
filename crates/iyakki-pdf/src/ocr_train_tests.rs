//! The test fonts, the page drawing the OCR tests use, and the (ignored) program that makes `ocr_model.bin`.
//!
//! Training fonts (the shapes in the table come from these only): DejaVu Sans, Sans Bold and Sans Mono (the files in
//! `iyakki-shell/assets`, Bitstream Vera licence), Liberation Sans, Serif and Serif Bold (SIL OFL) and Noto Sans (SIL OFL).
//! The recognition tests read pages drawn in fonts that are NOT in the table (FreeSans, FreeSerif, Liberation Mono, Noto Serif) as
//! well as in the training fonts, so the numbers say how it does on a shape it has not seen. Make the table again with
//! `cargo test -p iyakki-pdf --lib train_the_model -- --ignored` on a machine that has the fonts.

use super::*;
use fontdue::{Font, FontSettings};
use std::path::PathBuf;

pub const LIB: &str = "/usr/share/fonts/truetype/liberation";
pub const NOTO: &str = "/usr/share/fonts/truetype/noto";
pub const FREE: &str = "/usr/share/fonts/truetype/freefont";

pub fn assets(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../iyakki-shell/assets").join(name)
}

pub fn load(p: impl AsRef<std::path::Path>) -> Option<Font> {
    let b = std::fs::read(p).ok()?;
    Font::from_bytes(b, FontSettings::default()).ok()
}

pub fn training_fonts() -> Vec<(String, Font)> {
    let list: Vec<(String, PathBuf)> = vec![
        ("DejaVu Sans".into(), assets("DejaVuSans.ttf")),
        ("DejaVu Sans Bold".into(), assets("DejaVuSans-Bold.ttf")),
        ("DejaVu Sans Mono".into(), assets("DejaVuSansMono.ttf")),
        ("Liberation Sans".into(), format!("{LIB}/LiberationSans-Regular.ttf").into()),
        ("Liberation Serif".into(), format!("{LIB}/LiberationSerif-Regular.ttf").into()),
        ("Liberation Serif Bold".into(), format!("{LIB}/LiberationSerif-Bold.ttf").into()),
        ("Noto Sans".into(), format!("{NOTO}/NotoSans-Regular.ttf").into()),
    ];
    list.into_iter().filter_map(|(n, p)| load(p).map(|f| (n, f))).collect()
}

pub fn heldout_fonts() -> Vec<(String, Font)> {
    let list: Vec<(String, PathBuf)> = vec![
        ("FreeSans".into(), format!("{FREE}/FreeSans.ttf").into()),
        ("FreeSerif".into(), format!("{FREE}/FreeSerif.ttf").into()),
        ("Liberation Mono".into(), format!("{LIB}/LiberationMono-Regular.ttf").into()),
        ("Noto Serif".into(), format!("{NOTO}/NotoSerif-Regular.ttf").into()),
    ];
    list.into_iter().filter_map(|(n, p)| load(p).map(|f| (n, f))).collect()
}

/// Draws `text` with its baseline at `base` and the pen at `x`; returns the pen's end.
pub fn draw(img: &mut Gray, font: &Font, text: &str, px: f32, x: f32, base: i32) -> f32 {
    let mut pen = x;
    let mut prev: Option<char> = None;
    for ch in text.chars() {
        if let Some(p) = prev {
            pen += font.horizontal_kern(p, ch, px).unwrap_or(0.0);
        }
        let (m, bm) = font.rasterize(ch, px);
        let gx = pen.round() as i32 + m.xmin;
        let gy = base - m.ymin - m.height as i32;
        for yy in 0..m.height {
            for xx in 0..m.width {
                let (ix, iy) = (gx + xx as i32, gy + yy as i32);
                if ix < 0 || iy < 0 || ix as usize >= img.w || iy as usize >= img.h {
                    continue;
                }
                let o = iy as usize * img.w + ix as usize;
                let cov = bm[yy * m.width + xx] as i32;
                img.px[o] = (img.px[o] as i32 - cov).clamp(0, 255) as u8;
            }
        }
        pen += m.advance_width;
        prev = Some(ch);
    }
    pen
}

/// A page of text lines: `px` pixels per em, `lead` times that between baselines.
pub fn page(font: &Font, lines: &[&str], px: f32, lead: f32, margin: usize) -> Gray {
    let widest = lines.iter().map(|l| l.chars().map(|c| font.metrics(c, px).advance_width).sum::<f32>()).fold(0.0, f32::max);
    let w = widest as usize + 2 * margin + 4;
    let h = ((lines.len() as f32 + 1.0) * px * lead) as usize + 2 * margin;
    let mut img = Gray { w, h, px: vec![255; w * h] };
    for (i, l) in lines.iter().enumerate() {
        let base = margin as f32 + (i as f32 + 1.0) * px * lead;
        draw(&mut img, font, l, px, margin as f32, base as i32);
    }
    img
}

/// The same pipeline the reader runs, for one letter drawn alone: its features, or None when it is not one piece.
fn alone(font: &Font, ch: char, px: f32) -> Option<Template> {
    let (w, h) = ((px * 3.0) as usize, (px * 3.0) as usize);
    let base = (px * 2.0) as i32;
    let mut img = Gray { w, h, px: vec![255; w * h] };
    draw(&mut img, font, &ch.to_string(), px, px, base);
    let mut l_img = Gray { w, h, px: vec![255; w * h] };
    draw(&mut l_img, font, "l", px, px, base);
    let (_, lm) = ink_of(&l_img);
    let top = (0..h).find(|y| (0..w).any(|x| lm[y * w + x]))? as f32;
    let tall = base as f32 - top;
    let (ink, mask) = ink_of(&img);
    let mut labels = vec![];
    let mut ps = pieces(&mask, w, h, &mut labels)?;
    ps.retain(|p| p.area >= 3);
    let gl = join_stacked(ps, (tall * 0.6) as i32);
    if gl.len() != 1 {
        return None;
    }
    let f = features(&ink, &labels, w, &gl[0], base as f32, tall);
    Some(Template { ch, meta: f.meta, bmp: f.bmp })
}

pub const ALPHABET: &str = "!\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

#[test]
#[ignore = "makes src/ocr_model.bin from the fonts on this machine"]
fn train_the_model() {
    let fonts = training_fonts();
    assert!(fonts.len() >= 6, "the training fonts are not all here");
    let mut t = vec![];
    let mut skipped = std::collections::BTreeSet::new();
    for (_, f) in &fonts {
        for px in [18.0f32, 26.0, 38.0, 56.0] {
            for ch in ALPHABET.chars() {
                match alone(f, ch, px) {
                    Some(x) => t.push(x),
                    None => {
                        skipped.insert(ch);
                    }
                }
            }
        }
    }
    println!("{} templates; letters that are not one piece (read as their parts): {:?}", t.len(), skipped);
    std::fs::write(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/ocr_model.bin"), Model::write(&t)).unwrap();
}

#[test]
#[ignore = "prints how a line is measured"]
fn show_measure() {
    let f = &training_fonts()[1].1;
    let img = page(f, &["The quick brown fox jumps over the lazy dog."], 44.0, 1.5, 30);
    let p = recognise(&img);
    println!("tall {} baseline {} text {}", p.lines[0].height, p.lines[0].baseline, p.text());
}

#[test]
#[ignore = "prints the nearest shapes of one letter"]
fn show_nearest() {
    let m = Model::parse(MODEL).unwrap();
    for (name, f) in &training_fonts()[1..2] {
        for px in [31.0f32, 44.0] { // between the sizes of the table
            for ch in ['s', 'S'] {
                let t = alone(f, ch, px).unwrap();
                let feat = Feat { bmp: t.bmp, meta: t.meta };
                let mut v: Vec<(f32, char, [f32; 5])> = m.t.iter().map(|x| (feat.distance(x), x.ch, x.meta)).collect();
                v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                println!("{name} {px} {ch}: meta {:?} -> {:?}", t.meta, &v[..4]);
            }
        }
    }
}
