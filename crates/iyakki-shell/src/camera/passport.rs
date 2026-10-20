//! Passport and visa photos: the common sizes with their head-size rules, the crop at 300 dpi, the guide lines drawn
//! over the live picture, and a printable sheet (4×6 in photo paper or A4) with as many copies as fit, as a PNG and a PDF.
//!
//! The head rules are the published ones as of 2026 (crown to chin); authorities change them, so the app says to check.

use super::pixels::RectF;

/// Print resolution of every passport file.
pub const DPI: f32 = 300.0;
/// Space kept free at the page edges and between copies on a sheet (mm).
pub const SHEET_MARGIN_MM: f32 = 3.0;
pub const SHEET_GAP_MM: f32 = 2.0;

pub use super::specs::{spec, Digital, Spec, SPECS};

pub fn mm_to_px(mm: f32) -> u32 {
    (mm / 25.4 * DPI).round() as u32
}

impl Spec {
    /// The photo's size in pixels at 300 dpi.
    pub fn out_px(&self) -> (u32, u32) {
        (mm_to_px(self.w_mm), mm_to_px(self.h_mm))
    }
    pub fn aspect(&self) -> f32 {
        self.w_mm / self.h_mm
    }
}

/// The part of a `fw`×`fh` frame that becomes the photo: the spec's shape, `zoom` (0.2..=1) of the largest such
/// rectangle that fits, centred at (`cx`, `cy`) (fractions of the frame), moved back inside the frame when needed.
pub fn crop_rect(s: &Spec, fw: u32, fh: u32, zoom: f32, cx: f32, cy: f32) -> RectF {
    let (fw, fh) = (fw.max(1) as f32, fh.max(1) as f32);
    let a = s.aspect();
    let (mut w, mut h) = if fw / fh > a { (fh * a, fh) } else { (fw, fw / a) };
    let z = zoom.clamp(0.2, 1.0);
    w *= z;
    h *= z;
    let x = (cx * fw - w / 2.0).clamp(0.0, fw - w);
    let y = (cy * fh - h / 2.0).clamp(0.0, fh - h);
    RectF { x, y, w, h }
}

/// Where the guide lines go, as fractions of the photo's height (from the top) and width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guides {
    pub crown: f32,
    /// The chin should land between these two lines.
    pub chin_min: f32,
    pub chin_max: f32,
    /// The eye line (eyes sit a little under half way down the head).
    pub eye: f32,
    /// The head oval's width, as a fraction of the photo's width.
    pub head_w: f32,
}

pub fn guides(s: &Spec) -> Guides {
    let mid = (s.head_min_mm + s.head_max_mm) / 2.0;
    let crown = s.crown_mm();
    Guides {
        crown: crown / s.h_mm,
        chin_min: ((crown + s.head_min_mm) / s.h_mm).min(0.995),
        chin_max: ((crown + s.head_max_mm) / s.h_mm).min(0.999),
        eye: match (s.eye_min_mm, s.eye_max_mm) {
            (Some(a), Some(b)) => 1.0 - (a + b) / 2.0 / s.h_mm,
            _ => (crown + mid * 0.47) / s.h_mm,
        },
        head_w: (mid * 0.74) / s.w_mm,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paper {
    Photo4x6,
    A4,
}

impl Paper {
    pub fn name(self) -> &'static str {
        match self {
            Paper::Photo4x6 => "4×6 in photo paper",
            Paper::A4 => "A4",
        }
    }
    /// Portrait size in mm.
    pub fn mm(self) -> (f32, f32) {
        match self {
            Paper::Photo4x6 => (101.6, 152.4),
            Paper::A4 => (210.0, 297.0),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Paper::Photo4x6 => "4x6",
            Paper::A4 => "a4",
        }
    }
}

/// Copies on a page: the page size and the top-left corner of each copy, in mm from the page's top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub page_mm: (f32, f32),
    pub photo_mm: (f32, f32),
    pub cells: Vec<(f32, f32)>,
    pub landscape: bool,
}

impl Sheet {
    pub fn page_px(&self) -> (u32, u32) {
        (mm_to_px(self.page_mm.0), mm_to_px(self.page_mm.1))
    }
}

fn grid(page: (f32, f32), photo: (f32, f32)) -> (usize, usize) {
    let fit = |avail: f32, p: f32| (((avail - 2.0 * SHEET_MARGIN_MM + SHEET_GAP_MM) / (p + SHEET_GAP_MM)).floor().max(0.0)) as usize;
    (fit(page.0, photo.0), fit(page.1, photo.1))
}

/// The most copies of `s` that fit on `paper`, upright or turned (an equal count keeps the page upright); the block of
/// copies is centred on the page.
pub fn sheet_layout(s: &Spec, paper: Paper) -> Sheet {
    let (pw, ph) = paper.mm();
    let photo = (s.w_mm, s.h_mm);
    let (c1, r1) = grid((pw, ph), photo);
    let (c2, r2) = grid((ph, pw), photo);
    let landscape = c2 * r2 > c1 * r1;
    let (page, cols, rows) = if landscape { ((ph, pw), c2, r2) } else { ((pw, ph), c1, r1) };
    let bw = cols as f32 * photo.0 + cols.saturating_sub(1) as f32 * SHEET_GAP_MM;
    let bh = rows as f32 * photo.1 + rows.saturating_sub(1) as f32 * SHEET_GAP_MM;
    let (x0, y0) = ((page.0 - bw) / 2.0, (page.1 - bh) / 2.0);
    let mut cells = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            cells.push((x0 + c as f32 * (photo.0 + SHEET_GAP_MM), y0 + r as f32 * (photo.1 + SHEET_GAP_MM)));
        }
    }
    Sheet { page_mm: page, photo_mm: photo, cells, landscape }
}

/// The sheet as RGBA pixels at 300 dpi: white paper, the copies, and thin grey cutting lines around each.
pub fn sheet_rgba(sheet: &Sheet, photo_w: u32, photo_h: u32, photo: &[u8]) -> (u32, u32, Vec<u8>) {
    let (w, h) = sheet.page_px();
    let mut out = vec![255u8; w as usize * h as usize * 4];
    let (cw, ch) = (mm_to_px(sheet.photo_mm.0), mm_to_px(sheet.photo_mm.1));
    let scaled = if (cw, ch) == (photo_w, photo_h) { photo.to_vec() } else { super::pixels::crop_scale(photo_w, photo_h, photo, RectF { x: 0.0, y: 0.0, w: photo_w as f32, h: photo_h as f32 }, cw, ch, false) };
    let put = |out: &mut Vec<u8>, x: i64, y: i64, c: [u8; 4]| {
        if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
            let i = (y as usize * w as usize + x as usize) * 4;
            out[i..i + 4].copy_from_slice(&c);
        }
    };
    for (xm, ym) in &sheet.cells {
        let (x0, y0) = (mm_to_px(*xm) as i64, mm_to_px(*ym) as i64);
        for y in 0..ch as i64 {
            for x in 0..cw as i64 {
                let i = (y as usize * cw as usize + x as usize) * 4;
                put(&mut out, x0 + x, y0 + y, [scaled[i], scaled[i + 1], scaled[i + 2], 255]);
            }
        }
        // a hairline frame one pixel outside the photo
        let grey = [170, 170, 170, 255];
        for x in -1..=cw as i64 {
            put(&mut out, x0 + x, y0 - 1, grey);
            put(&mut out, x0 + x, y0 + ch as i64, grey);
        }
        for y in -1..=ch as i64 {
            put(&mut out, x0 - 1, y0 + y, grey);
            put(&mut out, x0 + cw as i64, y0 + y, grey);
        }
    }
    (w, h, out)
}

/// The sheet as a one-page PDF: the photo once (uncompressed RGB at 300 dpi), drawn at every copy's place at its true size,
/// with grey cutting frames. Written by hand: a catalog, a page, the image and the drawing, and the cross-reference table.
pub fn sheet_pdf(sheet: &Sheet, photo_w: u32, photo_h: u32, photo: &[u8]) -> Vec<u8> {
    let pt = |mm: f32| mm / 25.4 * 72.0;
    let (pw, ph) = (pt(sheet.page_mm.0), pt(sheet.page_mm.1));
    let (cw, ch) = (pt(sheet.photo_mm.0), pt(sheet.photo_mm.1));
    let mut draw = String::new();
    for (xm, ym) in &sheet.cells {
        let x = pt(*xm);
        let y = ph - pt(*ym) - ch;
        draw.push_str(&format!("q {cw:.3} 0 0 {ch:.3} {x:.3} {y:.3} cm /Im0 Do Q\n"));
        draw.push_str(&format!("q 0.67 G 0.3 w {:.3} {:.3} {:.3} {:.3} re S Q\n", x - 0.2, y - 0.2, cw + 0.4, ch + 0.4));
    }
    let rgb: Vec<u8> = photo.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offs = Vec::new();
    let mut obj = |out: &mut Vec<u8>, head: String, stream: Option<&[u8]>| {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{head}\n", offs.len()).as_bytes());
        if let Some(s) = stream {
            out.extend_from_slice(b"stream\n");
            out.extend_from_slice(s);
            out.extend_from_slice(b"\nendstream\n");
        }
        out.extend_from_slice(b"endobj\n");
    };
    obj(&mut out, "<< /Type /Catalog /Pages 2 0 R >>".into(), None);
    obj(&mut out, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None);
    obj(&mut out, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.3} {ph:.3}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>"), None);
    obj(&mut out, format!("<< /Type /XObject /Subtype /Image /Width {photo_w} /Height {photo_h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>", rgb.len()), Some(&rgb));
    obj(&mut out, format!("<< /Length {} >>", draw.len()), Some(draw.as_bytes()));
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offs.len() + 1).as_bytes());
    for o in &offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offs.len() + 1).as_bytes());
    out
}

/// A JPEG for an online form: the photo scaled into the allowed pixel range and compressed until it is within the file
/// size limits (quality searched from 95 down, then the picture made smaller if it is still too big).
pub fn digital_jpeg(d: &Digital, w: u32, h: u32, rgba: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    // no largest size given: up to four times the smallest
    let d = &Digital { max_px: if d.max_px == (0, 0) { (d.min_px.0 * 4, d.min_px.1 * 4) } else { d.max_px }, ..*d };
    let (wf, hf) = (w.max(1) as f32, h.max(1) as f32);
    let lo = (d.min_px.0 as f32 / wf).max(d.min_px.1 as f32 / hf);
    let hi = (d.max_px.0 as f32 / wf).min(d.max_px.1 as f32 / hf);
    let mut k = 1.0f32.clamp(lo.min(hi), hi.max(lo));
    let fit = |k: f32| ((wf * k).round().clamp(d.min_px.0 as f32, d.max_px.0 as f32) as u32, (hf * k).round().clamp(d.min_px.1 as f32, d.max_px.1 as f32) as u32);
    let (max_b, min_b) = (d.max_kb as usize * 1024, d.min_kb as usize * 1024);
    let mut grown = false;
    for _ in 0..16 {
        let (tw, th) = fit(k);
        let px = if (tw, th) == (w, h) { rgba.to_vec() } else { super::pixels::crop_scale(w, h, rgba, RectF { x: 0.0, y: 0.0, w: wf, h: hf }, tw, th, false) };
        // the best quality (100 down to 20) that is not too big
        let mut best: Option<Vec<u8>> = None;
        for q in [100u8, 97, 95, 90, 85, 80, 75, 70, 65, 60, 55, 50, 45, 40, 35, 30, 25, 20] {
            let j = super::jpeg::encode_rgba(tw, th, &px, q);
            if j.len() <= max_b {
                best = Some(j);
                break;
            }
        }
        match best {
            Some(j) if j.len() >= min_b => return Ok((tw, th, j)),
            Some(_) => {
                // under the minimum even at full quality: more pixels, if the rules allow
                let (nw, nh) = fit(k * 1.3);
                if (nw, nh) == (tw, th) {
                    return Err(format!("could not reach the {} KB minimum within the allowed size", d.min_kb));
                }
                k *= 1.3;
                grown = true;
            }
            None => {
                let (mw, mh) = fit(k * 0.85);
                if (mw, mh) == (tw, th) || grown {
                    break;
                }
                k *= 0.85;
            }
        }
    }
    Err(format!("could not get the photo under {} KB within the allowed size", d.max_kb))
}

/// What [`save`] wrote.
#[derive(Clone, Debug)]
pub struct Saved {
    pub photo: std::path::PathBuf,
    pub sheet_png: Option<std::path::PathBuf>,
    pub sheet_pdf: Option<std::path::PathBuf>,
    pub digital: Option<std::path::PathBuf>,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// The camera picture had fewer pixels than the print needs (it was enlarged).
    pub low_res: bool,
}

/// Cut a passport photo out of a frame and save it in `dir`: the 300 dpi PNG, the online-form JPEG when the rules have
/// one, and a print sheet when `paper` is given. Names start with `passport-<id>-<stamp>`; nothing is overwritten.
#[allow(clippy::too_many_arguments)]
pub fn save(dir: &std::path::Path, s: &Spec, fw: u32, fh: u32, rgba: &[u8], crop: RectF, paper: Option<Paper>, stamp: &str) -> Result<Saved, String> {
    let (ow, oh) = s.out_px();
    let photo = super::pixels::crop_scale(fw, fh, rgba, crop, ow, oh, false);
    let png = super::pixels::png_with_dpi(ow, oh, &photo, DPI)?;
    let stem = format!("passport-{}-{stamp}", s.id);
    let path = super::unique(dir, &stem, "png");
    std::fs::write(&path, &png).map_err(|e| format!("could not save the photo: {e}"))?;
    let digital = match &s.digital {
        Some(d) => {
            let (_, _, j) = digital_jpeg(d, ow, oh, &photo)?;
            let p = super::unique(dir, &format!("{stem}-online"), "jpg");
            std::fs::write(&p, j).map_err(|e| format!("could not save the online photo: {e}"))?;
            Some(p)
        }
        None => None,
    };
    let mut out = Saved { photo: path, sheet_png: None, sheet_pdf: None, digital, w: ow, h: oh, rgba: photo, low_res: crop.h < oh as f32 * 0.6 };
    if let Some(paper) = paper {
        let (a, b, _) = save_sheet(dir, s, paper, out.w, out.h, &out.rgba, stamp)?;
        out.sheet_png = Some(a);
        out.sheet_pdf = Some(b);
    }
    Ok(out)
}

/// A print sheet of a passport photo as PNG and PDF in `dir`: the two paths and the number of copies.
pub fn save_sheet(dir: &std::path::Path, s: &Spec, paper: Paper, w: u32, h: u32, rgba: &[u8], stamp: &str) -> Result<(std::path::PathBuf, std::path::PathBuf, usize), String> {
    let sheet = sheet_layout(s, paper);
    let (sw, sh, px) = sheet_rgba(&sheet, w, h, rgba);
    let pdf = sheet_pdf(&sheet, w, h, rgba);
    let stem = format!("passport-sheet-{}-{}-{stamp}", s.id, paper.id());
    let png_path = super::unique(dir, &stem, "png");
    let pdf_path = super::unique(dir, &stem, "pdf");
    std::fs::write(&png_path, super::pixels::png_with_dpi(sw, sh, &px, DPI)?).map_err(|e| e.to_string())?;
    std::fs::write(&pdf_path, pdf).map_err(|e| e.to_string())?;
    Ok((png_path, pdf_path, sheet.cells.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_size_crops_to_its_shape_at_300_dpi() {
        let want = [("eu-schengen", (413, 531)), ("uk-passport", (413, 531)), ("in-passport", (413, 531)), ("us-passport", (600, 600)), ("us-visa", (600, 600)), ("ca-passport", (591, 827)), ("cn-visa", (390, 567)), ("my-passport", (413, 591))];
        for (id, px) in want {
            assert_eq!(spec(id).unwrap().out_px(), px, "{id}");
        }
        for s in SPECS {
            let id = s.id;
            // a 1280x720 frame: the photo shape at full height, centred
            let r = crop_rect(s, 1280, 720, 1.0, 0.5, 0.5);
            assert!((r.h - 720.0).abs() < 0.01 && (r.w / r.h - s.aspect()).abs() < 1e-4, "{id}: {r:?}");
            assert!((r.x + r.w / 2.0 - 640.0).abs() < 0.01);
            // zoomed in and pushed to a corner: same shape, still inside the frame
            let r = crop_rect(s, 1280, 720, 0.5, 0.0, 1.0);
            assert!((r.h - 360.0).abs() < 0.01 && r.x == 0.0 && (r.y + r.h - 720.0).abs() < 0.01, "{id}: {r:?}");
            // a tall (portrait) frame is limited by its width
            let r = crop_rect(s, 480, 1000, 1.0, 0.5, 0.5);
            assert!((r.w - 480.0).abs() < 0.01 && r.y >= 0.0 && r.y + r.h <= 1000.0);
            // the guides are in order and inside the photo, and the head rules fit
            let g = guides(s);
            assert!(0.0 < g.crown && g.crown < g.eye && g.eye < g.chin_min && g.chin_min < g.chin_max && g.chin_max < 1.0, "{id}: {g:?}");
            assert!(g.head_w > 0.3 && g.head_w < 0.9, "{id}: {g:?}");
            assert!(s.crown_mm() + s.head_max_mm < s.h_mm, "{id}");
        }
        // China's crown rule (3–5 mm from the top) is followed
        assert!((guides(spec("cn-passport").unwrap()).crown * 48.0 - 4.0).abs() < 0.01);
        // zoom is kept within reason
        assert_eq!(crop_rect(&SPECS[0], 100, 100, 0.0, 0.5, 0.5), crop_rect(&SPECS[0], 100, 100, 0.2, 0.5, 0.5));
    }

    fn check_sheet(s: &Spec, paper: Paper) -> Sheet {
        let sh = sheet_layout(s, paper);
        let (pw, ph) = sh.page_mm;
        for (i, (x, y)) in sh.cells.iter().enumerate() {
            assert!(*x >= SHEET_MARGIN_MM - 1e-3 && *y >= SHEET_MARGIN_MM - 1e-3, "{} on {paper:?}: copy {i} in the margin", s.id);
            assert!(x + s.w_mm <= pw - SHEET_MARGIN_MM + 1e-3 && y + s.h_mm <= ph - SHEET_MARGIN_MM + 1e-3, "{} on {paper:?}: copy {i} off the page", s.id);
            for (j, (x2, y2)) in sh.cells.iter().enumerate().skip(i + 1) {
                let apart = (x2 - x).abs() >= s.w_mm + SHEET_GAP_MM - 1e-3 || (y2 - y).abs() >= s.h_mm + SHEET_GAP_MM - 1e-3;
                assert!(apart, "{} on {paper:?}: copies {i} and {j} touch", s.id);
            }
        }
        sh
    }

    #[test]
    fn the_print_sheets_hold_the_most_copies_that_fit_with_margins_and_gaps() {
        let eu = spec("eu-schengen").unwrap();
        let us = spec("us-passport").unwrap();
        let s = check_sheet(eu, Paper::Photo4x6);
        assert_eq!((s.cells.len(), s.landscape), (8, true), "4 x 2 on a turned 6x4");
        assert_eq!(s.page_px(), (1800, 1200));
        let s = check_sheet(us, Paper::Photo4x6);
        assert_eq!((s.cells.len(), s.landscape), (2, false));
        let s = check_sheet(eu, Paper::A4);
        assert_eq!((s.cells.len(), s.landscape), (30, false));
        assert_eq!(s.page_px(), (2480, 3508));
        let s = check_sheet(us, Paper::A4);
        assert_eq!(s.cells.len(), 15);
        for sp in SPECS {
            for p in [Paper::Photo4x6, Paper::A4] {
                assert!(!check_sheet(sp, p).cells.is_empty());
            }
        }
        // the block of copies is centred
        let s = sheet_layout(eu, Paper::Photo4x6);
        let right = s.page_mm.0 - (s.cells.last().unwrap().0 + eu.w_mm);
        assert!((s.cells[0].0 - right).abs() < 1e-3);
    }

    #[test]
    fn the_sheet_png_and_pdf_place_every_copy() {
        let eu = spec("eu-schengen").unwrap();
        let (w, h) = eu.out_px();
        let photo: Vec<u8> = (0..w * h).flat_map(|_| [10u8, 20, 200, 255]).collect();
        let sheet = sheet_layout(eu, Paper::Photo4x6);
        let (pw, ph, rgba) = sheet_rgba(&sheet, w, h, &photo);
        assert_eq!((pw, ph), (1800, 1200));
        let at = |x: u32, y: u32| &rgba[((y * pw + x) * 4) as usize..][..4];
        assert_eq!(at(2, 2), &[255, 255, 255, 255], "the margin is white paper");
        for (xm, ym) in &sheet.cells {
            let (x, y) = (mm_to_px(*xm) + w / 2, mm_to_px(*ym) + h / 2);
            assert_eq!(at(x, y), &[10, 20, 200, 255]);
            assert_eq!(at(mm_to_px(*xm) - 1, y), &[170, 170, 170, 255], "cutting line");
        }
        let pdf = sheet_pdf(&sheet, w, h, &photo);
        let find = |hay: &[u8], n: &[u8]| hay.windows(n.len()).position(|x| x == n);
        let text = String::from_utf8_lossy(&pdf);
        assert!(pdf.starts_with(b"%PDF-1.4") && pdf.ends_with(b"%%EOF\n"));
        assert_eq!(text.matches("/Im0 Do").count(), 8);
        assert!(text.contains("/MediaBox [0 0 432.000 288.000]"), "a turned 6x4 in page in points");
        assert!(text.contains(&format!("/Width {w} /Height {h}")));
        // the cross-reference table points at each object (byte offsets)
        let sx = pdf.windows(10).rposition(|x| x == b"startxref\n").unwrap();
        let xref: usize = std::str::from_utf8(&pdf[sx + 10..]).unwrap().lines().next().unwrap().parse().unwrap();
        let table = std::str::from_utf8(&pdf[xref..]).unwrap();
        assert!(table.starts_with("xref\n0 6\n"));
        for (n, line) in table.lines().skip(3).take(5).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(pdf[off..].starts_with(format!("{} 0 obj", n + 1).as_bytes()), "object {}", n + 1);
            assert_eq!(line.len(), 19, "entries are 20 bytes with the newline");
        }
        // the image stream is exactly the photo's RGB
        let len = (w * h * 3) as usize;
        let img_start = find(&pdf, b"/Subtype /Image").unwrap();
        let s2 = img_start + find(&pdf[img_start..], b"stream\n").unwrap() + 7;
        assert_eq!(&pdf[s2..s2 + 3], &[10, 20, 200]);
        assert_eq!(&pdf[s2 + len..s2 + len + 10], b"\nendstream");
    }

    /// A photo-like picture with fine detail, so JPEG sizes are realistic.
    fn textured(w: u32, h: u32) -> Vec<u8> {
        let mut x = 12345u32;
        (0..w * h).flat_map(|i| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = ((x >> 16) & 63) as u8;
            let (px, py) = (i % w, i / w);
            [(px * 180 / w) as u8 + n, (py * 180 / h) as u8 + n / 2, 120 + n, 255]
        }).collect()
    }

    #[test]
    fn online_photos_meet_each_countrys_pixel_and_file_size_limits() {
        let mut checked = 0;
        for s in SPECS.iter().filter(|s| s.digital.is_some()) {
            let d = s.digital.unwrap();
            let (w, h) = s.out_px();
            let (tw, th, j) = digital_jpeg(&d, w, h, &textured(w, h)).unwrap_or_else(|e| panic!("{}: {e}", s.id));
            let max = if d.max_px == (0, 0) { (d.min_px.0 * 4, d.min_px.1 * 4) } else { d.max_px };
            assert!(tw >= d.min_px.0 && th >= d.min_px.1 && tw <= max.0 && th <= max.1, "{}: {tw}x{th}", s.id);
            assert!(j.len() <= d.max_kb as usize * 1024 && j.len() >= d.min_kb as usize * 1024, "{}: {} bytes", s.id, j.len());
            let b = iyakki_web::jpeg::decode(&j).unwrap();
            assert_eq!((b.width, b.height), (tw, th));
            checked += 1;
        }
        assert!(checked >= 8, "{checked}");
        // the US visa: square, 600 to 1200 px, at most 240 KB
        let d = spec("us-visa").unwrap().digital.unwrap();
        assert_eq!((d.min_px, d.max_px, d.max_kb), ((600, 600), (1200, 1200), 240));
        // a limit that cannot be met is reported, not ignored
        let tiny = Digital { min_px: (2000, 2000), max_px: (2000, 2000), min_kb: 0, max_kb: 1, note: "" };
        assert!(digital_jpeg(&tiny, 100, 100, &textured(100, 100)).is_err());
    }

    #[test]
    fn save_writes_the_photo_the_online_file_and_the_sheet() {
        let d = std::env::temp_dir().join(format!("iyakki-pp-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let s = spec("us-visa").unwrap();
        let rgba = textured(1280, 720);
        let crop = crop_rect(s, 1280, 720, 0.9, 0.5, 0.5);
        let out = save(&d, s, 1280, 720, &rgba, crop, Some(Paper::Photo4x6), "20261018-120000").unwrap();
        assert!(out.photo.ends_with("passport-us-visa-20261018-120000.png"));
        let j = std::fs::read(out.digital.as_ref().unwrap()).unwrap();
        assert!(j.len() <= 240 * 1024 && j.starts_with(&[0xFF, 0xD8]));
        assert!(out.sheet_png.as_ref().unwrap().exists() && out.sheet_pdf.as_ref().unwrap().exists());
        assert!(!out.low_res);
        // the same stamp again: new names, nothing overwritten
        let again = save(&d, s, 1280, 720, &rgba, crop, None, "20261018-120000").unwrap();
        assert_ne!(again.photo, out.photo);
        assert!(again.sheet_png.is_none());
        // a country without online rules writes no online file
        let uk = spec("au-passport").unwrap();
        assert!(save(&d, uk, 1280, 720, &rgba, crop_rect(uk, 1280, 720, 0.9, 0.5, 0.5), None, "x").unwrap().digital.is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
