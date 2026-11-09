//! The document: page tree, page boxes, metadata, text and rendering entry points, form fields.

use crate::file::{File, PdfError};
use crate::geom::Matrix;
use crate::interp::{GState, Interp, Report};
use crate::obj::*;
use crate::render::{GlyphSource, RenderSink};
use crate::text::{self, Hit, TextLine, TextSink};
use std::collections::HashSet;
use std::rc::Rc;

const MAX_PAGES: usize = 50_000;
const MAX_RENDER_PIXELS: u64 = 36_000_000;

#[derive(Clone)]
pub struct Page {
    pub dict: Rc<Dict>,
    pub res: Rc<Dict>,
    /// The visible box (CropBox within MediaBox): x0, y0, x1, y1 in user space.
    pub bbox: [f64; 4],
    pub rotate: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub creator: String,
    pub producer: String,
    pub version: String,
    pub pages: usize,
    pub form_fields: usize,
    /// The file's structure had to be rebuilt by scanning.
    pub repaired: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FormField {
    pub name: String,
    pub kind: String,
    pub value: String,
}

pub struct Document {
    pub file: File,
    pages: Vec<Page>,
}

/// A PDF text string (UTF-16BE with a byte order mark, UTF-8 with one, or PDFDocEncoding which is Latin-1 for what matters here).
pub fn text_string(b: &[u8]) -> String {
    if b.starts_with(&[0xfe, 0xff]) {
        let units: Vec<u16> = b[2..].chunks(2).map(|c| if c.len() == 2 { (c[0] as u16) << 8 | c[1] as u16 } else { c[0] as u16 }).collect();
        String::from_utf16_lossy(&units)
    } else if b.starts_with(&[0xef, 0xbb, 0xbf]) {
        String::from_utf8_lossy(&b[3..]).into_owned()
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

fn rect(f: &File, o: &Obj) -> Option<[f64; 4]> {
    let a = f.resolve(o);
    let v: Vec<f64> = a.arr()?.iter().filter_map(|x| f.resolve(x).num()).collect();
    if v.len() < 4 || v.iter().any(|x| !x.is_finite() || x.abs() > 1e7) {
        return None;
    }
    Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

impl Document {
    pub fn open(bytes: Vec<u8>) -> Result<Document, PdfError> {
        let file = File::open(bytes)?;
        let mut d = Document { file, pages: vec![] };
        d.pages = d.collect_pages();
        if d.pages.is_empty() {
            return Err(PdfError::NoPages);
        }
        Ok(d)
    }

    fn collect_pages(&self) -> Vec<Page> {
        let f = &self.file;
        let mut out = Vec::new();
        let root = f.get(&f.trailer, "Root");
        if let Some(r) = root.dict() {
            let pages = r.get("Pages").cloned().unwrap_or(Obj::Null);
            let mut seen = HashSet::new();
            let inherited = Inherit { res: None, media: None, crop: None, rotate: 0 };
            self.walk(&pages, &inherited, &mut seen, 0, &mut out);
        }
        if out.is_empty() {
            // a damaged page tree: take every page object
            let mut seen = HashSet::new();
            for n in f.all_numbers() {
                if out.len() >= MAX_PAGES {
                    break;
                }
                let o = f.load(n);
                if let Some(d) = o.dict() {
                    if f.get(d, "Type").name() == Some("Page") && seen.insert(n) {
                        self.add_page(d, &Inherit { res: None, media: None, crop: None, rotate: 0 }, &mut out);
                    }
                }
            }
        }
        out
    }

    fn walk(&self, node: &Obj, inh: &Inherit, seen: &mut HashSet<u32>, depth: usize, out: &mut Vec<Page>) {
        if depth > 40 || out.len() >= MAX_PAGES {
            return;
        }
        if let Obj::Ref(n, _) = node {
            if !seen.insert(*n) {
                return;
            }
        }
        let f = &self.file;
        let o = f.resolve(node);
        let Some(d) = o.dict().cloned() else { return };
        let mut here = inh.clone();
        if let Some(r) = d.get("Resources") {
            if let Some(rd) = f.resolve(r).dict() {
                here.res = Some(rd.clone());
            }
        }
        if let Some(m) = d.get("MediaBox").and_then(|m| rect(f, m)) {
            here.media = Some(m);
        }
        if let Some(m) = d.get("CropBox").and_then(|m| rect(f, m)) {
            here.crop = Some(m);
        }
        if let Some(r) = f.get(&d, "Rotate").int() {
            here.rotate = r as i32;
        }
        let is_pages = f.get(&d, "Type").name() == Some("Pages") || d.contains_key("Kids");
        if is_pages {
            if let Some(kids) = f.get(&d, "Kids").arr() {
                for k in kids.iter() {
                    self.walk(k, &here, seen, depth + 1, out);
                }
            }
        } else {
            self.add_page(&d, &here, out);
        }
    }

    fn add_page(&self, d: &Rc<Dict>, inh: &Inherit, out: &mut Vec<Page>) {
        let f = &self.file;
        let mut here = inh.clone();
        if let Some(r) = d.get("Resources").map(|r| f.resolve(r)).and_then(|r| r.dict().cloned()) {
            here.res = Some(r);
        }
        let media = d.get("MediaBox").and_then(|m| rect(f, m)).or(here.media).unwrap_or([0.0, 0.0, 612.0, 792.0]);
        let crop = d.get("CropBox").and_then(|m| rect(f, m)).or(here.crop);
        let mut bbox = media;
        if let Some(c) = crop {
            let i = [c[0].max(media[0]), c[1].max(media[1]), c[2].min(media[2]), c[3].min(media[3])];
            if i[2] > i[0] && i[3] > i[1] {
                bbox = i;
            }
        }
        if bbox[2] - bbox[0] < 1.0 || bbox[3] - bbox[1] < 1.0 {
            bbox = [0.0, 0.0, 612.0, 792.0];
        }
        let rot = f.get(d, "Rotate").int().map(|r| r as i32).unwrap_or(here.rotate);
        let rotate = ((rot % 360) + 360) % 360 / 90 * 90;
        out.push(Page { dict: d.clone(), res: here.res.unwrap_or_default(), bbox, rotate });
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn page(&self, i: usize) -> Result<&Page, PdfError> {
        self.pages.get(i).ok_or(PdfError::PageOutOfRange)
    }

    /// Size of a page in points after rotation.
    pub fn page_size(&self, i: usize) -> Result<(f64, f64), PdfError> {
        let p = self.page(i)?;
        let (w, h) = (p.bbox[2] - p.bbox[0], p.bbox[3] - p.bbox[1]);
        Ok(if p.rotate % 180 == 90 { (h, w) } else { (w, h) })
    }

    /// The matrix from user space to pixels (y down) at `scale` pixels per point.
    pub fn page_matrix(&self, i: usize, s: f64) -> Result<Matrix, PdfError> {
        let p = self.page(i)?;
        let [x0, y0, x1, y1] = p.bbox;
        Ok(Matrix(match p.rotate {
            90 => [0.0, s, s, 0.0, -s * y0, -s * x0],
            180 => [-s, 0.0, 0.0, s, s * x1, -s * y0],
            270 => [0.0, -s, -s, 0.0, s * y1, s * x1],
            _ => [s, 0.0, 0.0, -s, -s * x0, s * y1],
        }))
    }

    fn contents(&self, p: &Page) -> Vec<u8> {
        let f = &self.file;
        let mut out = Vec::new();
        let c = f.get(&p.dict, "Contents");
        let list: Vec<Obj> = match &c {
            Obj::Arr(a) => a.iter().take(10_000).map(|x| f.resolve(x)).collect(),
            o @ Obj::Stream(..) => vec![o.clone()],
            _ => vec![],
        };
        for s in list {
            if let Ok(d) = f.stream_data(&s) {
                out.extend_from_slice(&d.data);
                out.push(b'\n');
            }
        }
        out
    }

    pub fn info(&self) -> Info {
        let f = &self.file;
        let d = f.get(&f.trailer, "Info");
        let s = |k: &str| d.dict().map(|d| f.get(d, k)).and_then(|o| o.str_bytes().map(text_string)).unwrap_or_default();
        let header = &f.data[..f.data.len().min(1100)];
        let version = header.windows(5).position(|w| w == b"%PDF-").map(|p| String::from_utf8_lossy(&header[p + 5..(p + 8).min(header.len())]).trim().to_string()).unwrap_or_default();
        Info {
            title: s("Title"),
            author: s("Author"),
            subject: s("Subject"),
            creator: s("Creator"),
            producer: s("Producer"),
            version,
            pages: self.pages.len(),
            form_fields: self.form_fields().len(),
            repaired: f.repaired,
        }
    }

    /// The text lines of a page, in content order, with positions (user space).
    pub fn page_lines(&self, i: usize) -> Result<(Vec<TextLine>, Report), PdfError> {
        let p = self.page(i)?;
        self.file.reset_budget();
        let data = self.contents(p);
        let mut it = Interp::new(&self.file);
        let mut sink = TextSink::default();
        it.run(&data, &p.res, GState::new(Matrix::ID), &mut sink, 0);
        // text in form XObjects is reached through Do; annotation text (form values) is not
        Ok((sink.lines, it.rep))
    }

    pub fn page_text(&self, i: usize) -> Result<String, PdfError> {
        let (lines, _) = self.page_lines(i)?;
        Ok(text::page_text(&lines))
    }

    /// All text, pages separated by form feeds, capped at `max_chars`.
    pub fn text(&self, max_chars: usize) -> String {
        let mut out = String::new();
        for i in 0..self.pages.len() {
            if out.chars().count() >= max_chars {
                break;
            }
            if i > 0 {
                out.push('\u{c}');
            }
            if let Ok(t) = self.page_text(i) {
                out.push_str(&t);
            }
        }
        out.chars().take(max_chars).collect()
    }

    /// Search one page; the boxes are in user space.
    pub fn search_page(&self, i: usize, query: &str, case_sensitive: bool) -> Result<Vec<Hit>, PdfError> {
        let (lines, _) = self.page_lines(i)?;
        Ok(text::find(&lines, query, case_sensitive))
    }

    /// Search every page: (page index, hit).
    pub fn search(&self, query: &str, case_sensitive: bool) -> Vec<(usize, Hit)> {
        let mut out = Vec::new();
        for i in 0..self.pages.len().min(3000) {
            if let Ok(h) = self.search_page(i, query, case_sensitive) {
                out.extend(h.into_iter().map(|h| (i, h)));
            }
            if out.len() >= 5000 {
                break;
            }
        }
        out
    }

    /// Render a page at `scale` pixels per point. Returns RGBA pixels and what could not be shown.
    pub fn render(&self, i: usize, scale: f64, glyphs: &dyn GlyphSource) -> Result<Rendered, PdfError> {
        let p = self.page(i)?;
        let (w, h) = self.page_size(i)?;
        let scale = if scale.is_finite() { scale.clamp(0.05, 16.0) } else { 1.0 };
        let (pw, ph) = ((w * scale).ceil().max(1.0) as u64, (h * scale).ceil().max(1.0) as u64);
        if pw * ph > MAX_RENDER_PIXELS {
            return Err(PdfError::TooLarge);
        }
        self.file.reset_budget();
        let m = self.page_matrix(i, scale)?;
        let data = self.contents(p);
        let mut it = Interp::new(&self.file);
        let mut sink = RenderSink::new(pw as u32, ph as u32, glyphs);
        let page_clip = {
            let [x0, y0, x1, y1] = p.bbox;
            let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
            let mut path = Vec::new();
            for (k, (x, y)) in pts.iter().enumerate() {
                let q = m.apply(*x, *y);
                path.push(if k == 0 { crate::geom::Seg::Move(q.0, q.1) } else { crate::geom::Seg::Line(q.0, q.1) });
            }
            path.push(crate::geom::Seg::Close);
            path
        };
        use crate::interp::Sink;
        sink.clip(&page_clip, false);
        it.run(&data, &p.res, GState::new(m), &mut sink, 0);
        self.draw_annotations(i, m, &mut it, &mut sink);
        let mut unsupported = it.rep.unsupported.clone();
        unsupported.extend(sink.notes.iter().cloned());
        if it.rep.stopped_early {
            unsupported.insert("a page too complex to draw completely".into());
        }
        Ok(Rendered { width: pw as u32, height: ph as u32, rgba: sink.canvas.rgba, unsupported: unsupported.into_iter().collect() })
    }

    fn draw_annotations(&self, i: usize, base: Matrix, it: &mut Interp, sink: &mut dyn crate::interp::Sink) {
        let f = &self.file;
        let Ok(p) = self.page(i) else { return };
        let Some(annots) = f.get(&p.dict, "Annots").arr().cloned() else { return };
        for a in annots.iter().take(5000) {
            let ao = f.resolve(a);
            let Some(ad) = ao.dict().cloned() else { continue };
            let flags = f.get(&ad, "F").int().unwrap_or(0);
            if flags & 2 != 0 || flags & 32 != 0 {
                continue; // hidden, no view
            }
            if f.get(&ad, "Subtype").name() == Some("Popup") {
                continue;
            }
            let Some(r) = ad.get("Rect").and_then(|r| rect(f, r)) else { continue };
            let ap = f.get(&ad, "AP");
            let Some(apd) = ap.dict() else { continue };
            let mut n = f.get(apd, "N");
            if let Obj::Dict(states) = &n {
                // a state dictionary: pick /AS
                let st = f.get(&ad, "AS").name().map(|s| s.to_string());
                n = match st.and_then(|s| states.get(&s).cloned()) {
                    Some(x) => f.resolve(&x),
                    None => Obj::Null,
                };
            }
            let Obj::Stream(fd, _, _) = &n else { continue };
            let bbox = f.get(fd, "BBox").arr().map(|a| a.iter().filter_map(|x| f.resolve(x).num()).collect::<Vec<_>>()).filter(|v| v.len() >= 4);
            let fm = f.get(fd, "Matrix").arr().map(|a| a.iter().filter_map(|x| f.resolve(x).num()).collect::<Vec<_>>()).and_then(|v| Matrix::from_arr(&v)).unwrap_or(Matrix::ID);
            let Some(bb) = bbox else { continue };
            let pts: Vec<(f64, f64)> = [(bb[0], bb[1]), (bb[2], bb[1]), (bb[2], bb[3]), (bb[0], bb[3])].iter().map(|(x, y)| fm.apply(*x, *y)).collect();
            let (tx0, tx1) = (pts.iter().map(|p| p.0).fold(f64::MAX, f64::min), pts.iter().map(|p| p.0).fold(f64::MIN, f64::max));
            let (ty0, ty1) = (pts.iter().map(|p| p.1).fold(f64::MAX, f64::min), pts.iter().map(|p| p.1).fold(f64::MIN, f64::max));
            if tx1 - tx0 < 1e-6 || ty1 - ty0 < 1e-6 {
                continue;
            }
            let (sx, sy) = ((r[2] - r[0]) / (tx1 - tx0), (r[3] - r[1]) / (ty1 - ty0));
            let a_m = Matrix([sx, 0.0, 0.0, sy, r[0] - tx0 * sx, r[1] - ty0 * sy]);
            let mut gs = GState::new(a_m.then(&base));
            gs.ctm = a_m.then(&base);
            it.draw_form(&n, fd, &p.res, &gs, sink, 1);
        }
    }

    /// The fields of the document's interactive form with their current values (shown, not edited).
    pub fn form_fields(&self) -> Vec<FormField> {
        let f = &self.file;
        let root = f.get(&f.trailer, "Root");
        let Some(r) = root.dict() else { return vec![] };
        let af = f.get(r, "AcroForm");
        let Some(afd) = af.dict() else { return vec![] };
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        if let Some(fields) = f.get(afd, "Fields").arr() {
            for fl in fields.iter() {
                self.field(fl, "", None, &mut out, &mut seen, 0);
            }
        }
        out
    }

    fn field(&self, node: &Obj, prefix: &str, ft: Option<String>, out: &mut Vec<FormField>, seen: &mut HashSet<u32>, depth: usize) {
        if depth > 20 || out.len() > 5000 {
            return;
        }
        if let Obj::Ref(n, _) = node {
            if !seen.insert(*n) {
                return;
            }
        }
        let f = &self.file;
        let o = f.resolve(node);
        let Some(d) = o.dict() else { return };
        let part = f.get(d, "T").str_bytes().map(text_string).unwrap_or_default();
        let name = if prefix.is_empty() { part.clone() } else if part.is_empty() { prefix.to_string() } else { format!("{prefix}.{part}") };
        let ft = f.get(d, "FT").name().map(|s| s.to_string()).or(ft);
        if let Some(kids) = f.get(d, "Kids").arr() {
            for k in kids.iter() {
                self.field(k, &name, ft.clone(), out, seen, depth + 1);
            }
            return;
        }
        let v = f.get(d, "V");
        let value = match &v {
            Obj::Str(s) => text_string(s),
            Obj::Name(n) => n.to_string(),
            Obj::Int(i) => i.to_string(),
            Obj::Arr(a) => a.iter().filter_map(|x| f.resolve(x).str_bytes().map(text_string)).collect::<Vec<_>>().join(", "),
            _ => String::new(),
        };
        let kind = match ft.as_deref() {
            Some("Tx") => "text",
            Some("Btn") => "button",
            Some("Ch") => "choice",
            Some("Sig") => "signature",
            _ => "field",
        };
        out.push(FormField { name, kind: kind.into(), value });
    }
}

#[derive(Clone)]
struct Inherit {
    res: Option<Rc<Dict>>,
    media: Option<[f64; 4]>,
    crop: Option<[f64; 4]>,
    rotate: i32,
}

pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Features on the page that are not shown (patterns, gradients, soft masks, pictures of unsupported kinds, ...).
    pub unsupported: Vec<String>,
}
