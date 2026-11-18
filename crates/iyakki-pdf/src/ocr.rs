//! Text recognition (OCR) for scanned pages, written in-house with no library and no downloaded model.
//!
//! The page picture is made black and white (Otsu's threshold), its connected pieces are found, grouped into text lines by the
//! empty rows between them, pieces that belong to one letter (the dot of an i, the two dots of a colon) are joined, and every letter
//! is compared with a table of letter shapes by nearest neighbour. The table ([`MODEL`], `ocr_model.bin`) is made by
//! `ocr_train_tests.rs` from three permissively licensed font families (DejaVu Sans, Liberation Sans and Serif, Noto Sans: bitmaps
//! of every printable ASCII letter and a few typographic marks, at two sizes), so what is shipped is data computed from outlines,
//! never a font file. A shape is described by a 12 x 16 grey bitmap of the letter (only its own pixels), and by where the letter
//! sits against the line's baseline and its tallest letters, so `o` and `O`, `,` and `.` are told apart by size and place.
//!
//! Limits (they are the row's remainder): Latin letters, digits and common punctuation of ASCII (no accents, no other scripts),
//! upright text in one or more columns read line by line, black on white or white on black; handwriting, tables and pictures of text
//! are not read. Touching letters are split at the thinnest column, which is wrong now and then. Skew up to about 3 degrees is
//! corrected. Hostile pictures are capped in size and in the number of pieces.

pub const MODEL: &[u8] = include_bytes!("ocr_model.bin");

pub const GW: usize = 12;
pub const GH: usize = 16;
pub const NB: usize = GW * GH;
/// Pictures with more pixels than this are refused.
pub const MAX_PIXELS: u64 = 60_000_000;
const MAX_COMPONENTS: usize = 300_000;

/// A grey picture, 0 black to 255 white, row by row.
#[derive(Clone, Debug)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Gray {
    pub fn new(w: usize, h: usize, px: Vec<u8>) -> Option<Gray> {
        (w > 0 && h > 0 && px.len() == w * h && (w as u64) * (h as u64) <= MAX_PIXELS).then_some(Gray { w, h, px })
    }

    /// From RGBA bytes (the renderer's output).
    pub fn from_rgba(w: usize, h: usize, rgba: &[u8]) -> Option<Gray> {
        if w == 0 || h == 0 || rgba.len() < w * h * 4 || (w as u64) * (h as u64) > MAX_PIXELS {
            return None;
        }
        let px = rgba.chunks_exact(4).take(w * h).map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8).collect();
        Some(Gray { w, h, px })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OcrWord {
    pub text: String,
    /// pixels, `x1` and `y1` exclusive
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    /// 0 (a guess) to 1 (a template of the same shape)
    pub confidence: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OcrLine {
    pub words: Vec<OcrWord>,
    /// the baseline, in pixels from the top
    pub baseline: f32,
    /// height of the tallest letters above the baseline
    pub height: f32,
}

impl OcrLine {
    /// the left edge of the first word
    pub fn x_start(&self) -> f64 {
        self.words.first().map(|w| w.x0 as f64).unwrap_or(0.0)
    }

    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OcrPage {
    pub lines: Vec<OcrLine>,
    pub w: usize,
    pub h: usize,
    /// degrees the picture was turned to make the lines straight
    pub skew: f32,
    /// pixels per point of the drawing that was read (set by the document, 0 for a bare picture)
    pub scale: f32,
}

impl OcrPage {
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.text()).collect::<Vec<_>>().join("\n")
    }

    pub fn mean_confidence(&self) -> f32 {
        let (mut s, mut n) = (0.0, 0);
        for w in self.lines.iter().flat_map(|l| &l.words) {
            s += w.confidence;
            n += 1;
        }
        if n == 0 { 0.0 } else { s / n as f32 }
    }
}

// ------------------------------------------------------------------------------------------------ the shape table

#[derive(Clone)]
pub struct Template {
    pub ch: char,
    pub meta: [f32; 5],
    pub bmp: [u8; NB],
}

pub struct Model {
    pub t: Vec<Template>,
}

impl Model {
    pub fn parse(b: &[u8]) -> Option<Model> {
        if b.len() < 8 || &b[..4] != b"IYOC" {
            return None;
        }
        let n = u32::from_le_bytes(b[4..8].try_into().ok()?) as usize;
        let rec = 4 + 10 + NB / 2;
        if b.len() != 8 + n * rec || n > 100_000 {
            return None;
        }
        let mut t = Vec::with_capacity(n);
        for r in b[8..].chunks_exact(rec) {
            let ch = char::from_u32(u32::from_le_bytes(r[..4].try_into().ok()?))?;
            let mut meta = [0f32; 5];
            for (i, m) in meta.iter_mut().enumerate() {
                *m = i16::from_le_bytes([r[4 + i * 2], r[5 + i * 2]]) as f32 / 1000.0;
            }
            let mut bmp = [0u8; NB];
            for (i, b) in r[14..].iter().enumerate() {
                bmp[2 * i] = (b & 15) * 17;
                bmp[2 * i + 1] = (b >> 4) * 17;
            }
            t.push(Template { ch, meta, bmp });
        }
        Some(Model { t })
    }

    pub fn write(t: &[Template]) -> Vec<u8> {
        let mut v = b"IYOC".to_vec();
        v.extend_from_slice(&(t.len() as u32).to_le_bytes());
        for x in t {
            v.extend_from_slice(&(x.ch as u32).to_le_bytes());
            for m in x.meta {
                v.extend_from_slice(&((m * 1000.0).round().clamp(-32000.0, 32000.0) as i16).to_le_bytes());
            }
            for p in x.bmp.chunks(2) {
                v.push(((p[0] as u16 + 8) / 17 + (((p[1] as u16 + 8) / 17) << 4)) as u8);
            }
        }
        v
    }

    /// The nearest template: its letter, the distance, and the distance of the nearest template of a different letter.
    pub fn nearest(&self, f: &Feat) -> (char, f32, f32) {
        let mut best = (' ', f32::MAX);
        let mut second = f32::MAX;
        for t in &self.t {
            let d = f.distance_below(t, second.max(best.1));
            if d < best.1 {
                if t.ch != best.0 {
                    second = best.1;
                }
                best = (t.ch, d);
            } else if d < second && t.ch != best.0 {
                second = d;
            }
        }
        (best.0, best.1, second)
    }
}

/// What is compared: the letter's bitmap and its place against the line.
#[derive(Clone)]
pub struct Feat {
    pub bmp: [u8; NB],
    pub meta: [f32; 5],
}

const META_W: [f32; 5] = [0.6, 4.0, 4.0, 3.0, 3.0];
const BMP_K: f32 = 24.0;

impl Feat {
    pub fn distance(&self, t: &Template) -> f32 {
        self.distance_below(t, f32::MAX)
    }

    /// The distance, or something larger than `limit` as soon as it is known to be larger.
    pub fn distance_below(&self, t: &Template, limit: f32) -> f32 {
        let mut meta = 0.0;
        for i in 0..5 {
            let d = self.meta[i] - t.meta[i];
            meta += META_W[i] * d * d;
        }
        if meta > 6.0 {
            return meta + 1.0;
        }
        let mut s = 0u32;
        let cap = if limit < f32::MAX { ((limit - meta).max(0.0) * NB as f32 * 255.0 * 255.0 / BMP_K) as u32 } else { u32::MAX };
        for i in 0..NB {
            let d = self.bmp[i] as i32 - t.bmp[i] as i32;
            s += (d * d) as u32;
            if s > cap && i % 16 == 15 {
                return limit + 1.0;
            }
        }
        meta + BMP_K * s as f32 / (NB as f32 * 255.0 * 255.0)
    }
}

// ------------------------------------------------------------------------------------------------ pieces

#[derive(Clone, Debug)]
pub struct Piece {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub area: u32,
    pub ids: Vec<u32>,
}

impl Piece {
    fn w(&self) -> i32 {
        self.x1 - self.x0 + 1
    }
    fn h(&self) -> i32 {
        self.y1 - self.y0 + 1
    }
}

/// The ink threshold (Otsu): pixels at or below it are ink.
pub fn otsu(px: &[u8]) -> u8 {
    let mut hist = [0u64; 256];
    for &p in px {
        hist[p as usize] += 1;
    }
    let total = px.len() as f64;
    let sum: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * *c as f64).sum();
    let (mut wb, mut sb, mut best, mut thr) = (0.0, 0.0, -1.0, 128u8);
    for t in 0..256 {
        wb += hist[t] as f64;
        if wb == 0.0 {
            continue;
        }
        let wf = total - wb;
        if wf == 0.0 {
            break;
        }
        sb += t as f64 * hist[t] as f64;
        let (mb, mf) = (sb / wb, (sum - sb) / wf);
        let v = wb * wf * (mb - mf) * (mb - mf);
        if v > best {
            best = v;
            thr = t as u8;
        }
    }
    thr
}

/// Connected pieces of the ink mask (8 neighbours). `labels` gets the piece id plus one (0: no ink).
pub fn pieces(mask: &[bool], w: usize, h: usize, labels: &mut Vec<u32>) -> Option<Vec<Piece>> {
    labels.clear();
    labels.resize(w * h, 0);
    let mut parent: Vec<u32> = vec![0];
    fn find(p: &mut [u32], mut a: u32) -> u32 {
        while p[a as usize] != a {
            p[a as usize] = p[p[a as usize] as usize];
            a = p[a as usize];
        }
        a
    }
    for y in 0..h {
        for x in 0..w {
            if !mask[y * w + x] {
                continue;
            }
            let mut near = [0u32; 4];
            let mut n = 0;
            if x > 0 {
                near[n] = labels[y * w + x - 1];
                n += 1;
            }
            if y > 0 {
                if x > 0 {
                    near[n] = labels[(y - 1) * w + x - 1];
                    n += 1;
                }
                near[n] = labels[(y - 1) * w + x];
                n += 1;
                if x + 1 < w {
                    near[n] = labels[(y - 1) * w + x + 1];
                    n += 1;
                }
            }
            let mut lab = 0u32;
            for &l in &near[..n] {
                if l != 0 {
                    if lab == 0 {
                        lab = find(&mut parent, l);
                    } else {
                        let r = find(&mut parent, l);
                        if r != lab {
                            let (lo, hi) = if r < lab { (r, lab) } else { (lab, r) };
                            parent[hi as usize] = lo;
                            lab = lo;
                        }
                    }
                }
            }
            if lab == 0 {
                if parent.len() > MAX_COMPONENTS * 4 {
                    return None;
                }
                lab = parent.len() as u32;
                parent.push(lab);
            }
            labels[y * w + x] = lab;
        }
    }
    let mut index: Vec<u32> = vec![u32::MAX; parent.len()];
    let mut out: Vec<Piece> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let l = labels[y * w + x];
            if l == 0 {
                continue;
            }
            let r = find(&mut parent, l) as usize;
            if index[r] == u32::MAX {
                if out.len() >= MAX_COMPONENTS {
                    return None;
                }
                index[r] = out.len() as u32;
                out.push(Piece { x0: x as i32, y0: y as i32, x1: x as i32, y1: y as i32, area: 0, ids: vec![out.len() as u32] });
            }
            let id = index[r];
            let p = &mut out[id as usize];
            p.x0 = p.x0.min(x as i32);
            p.x1 = p.x1.max(x as i32);
            p.y0 = p.y0.min(y as i32);
            p.y1 = p.y1.max(y as i32);
            p.area += 1;
            labels[y * w + x] = id + 1;
        }
    }
    Some(out)
}

// ------------------------------------------------------------------------------------------------ features

/// The bitmap and place of one letter made of `g.ids` pieces. `ink` is 0..255 per pixel (255: full ink).
pub fn features(ink: &[u8], labels: &[u32], w: usize, g: &Piece, baseline: f32, tall: f32) -> Feat {
    let (gw, gh) = (g.w() as usize, g.h() as usize);
    let mut bmp = [0u8; NB];
    for cy in 0..GH {
        let sy0 = cy * gh / GH;
        let sy1 = (((cy + 1) * gh).div_ceil(GH)).max(sy0 + 1).min(gh);
        for cx in 0..GW {
            let sx0 = cx * gw / GW;
            let sx1 = (((cx + 1) * gw).div_ceil(GW)).max(sx0 + 1).min(gw);
            let (mut s, mut n) = (0u32, 0u32);
            for sy in sy0..sy1 {
                let row = (g.y0 as usize + sy) * w + g.x0 as usize;
                for sx in sx0..sx1 {
                    n += 1;
                    let l = labels[row + sx];
                    if l != 0 && g.ids.contains(&(l - 1)) {
                        s += ink[row + sx] as u32;
                    }
                }
            }
            bmp[cy * GW + cx] = (s / n.max(1)) as u8;
        }
    }
    let tall = tall.max(1.0);
    let meta = [
        ((gw as f32) / (gh as f32)).log2().clamp(-4.0, 4.0),
        (baseline - g.y0 as f32) / tall,
        (baseline - (g.y1 + 1) as f32) / tall,
        gw as f32 / tall,
        gh as f32 / tall,
    ];
    Feat { bmp, meta }
}

fn model() -> &'static Model {
    static M: std::sync::OnceLock<Model> = std::sync::OnceLock::new();
    M.get_or_init(|| Model::parse(MODEL).unwrap_or(Model { t: vec![] }))
}

/// Joins pieces that are one letter: stacked pieces whose columns overlap (the dot of an i or j, the parts of a colon, semicolon,
/// question mark, exclamation mark and quotation marks).
pub fn join_stacked(mut v: Vec<Piece>, max_gap: i32) -> Vec<Piece> {
    v.sort_by_key(|p| p.x0);
    let mut out: Vec<Piece> = Vec::new();
    for p in v {
        let mut merged = false;
        for q in out.iter_mut().rev().take(4) {
            let ov = (p.x1.min(q.x1) - p.x0.max(q.x0) + 1).max(0);
            let narrow = p.w().min(q.w()).max(1);
            let vert_overlap = p.y0 <= q.y1 && q.y0 <= p.y1;
            let gap = if vert_overlap { 0 } else { (p.y0 - q.y1).max(q.y0 - p.y1) };
            let near = gap <= max_gap.max(3);
            let nested = (p.x0 >= q.x0 && p.x1 <= q.x1 && p.y0 >= q.y0 && p.y1 <= q.y1) || (q.x0 >= p.x0 && q.x1 <= p.x1 && q.y0 >= p.y0 && q.y1 <= p.y1);
            if nested || (ov * 2 >= narrow && !vert_overlap && near) {
                q.x0 = q.x0.min(p.x0);
                q.x1 = q.x1.max(p.x1);
                q.y0 = q.y0.min(p.y0);
                q.y1 = q.y1.max(p.y1);
                q.area += p.area;
                q.ids.extend(p.ids.iter().copied());
                merged = true;
                break;
            }
        }
        if !merged {
            out.push(p);
        }
    }
    out.sort_by_key(|p| p.x0);
    out
}

// ------------------------------------------------------------------------------------------------ the page

#[derive(Clone, Debug)]
struct Read {
    ch: char,
    conf: f32,
    x0: i32,
    x1: i32,
    y0: i32,
    y1: i32,
}

/// The tight box of the ink of `g` inside the columns `xa..=xb`.
fn sub_piece(labels: &[u32], w: usize, g: &Piece, xa: i32, xb: i32) -> Option<Piece> {
    let (mut x0, mut x1, mut y0, mut y1, mut area) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN, 0u32);
    for y in g.y0..=g.y1 {
        for x in xa.max(g.x0)..=xb.min(g.x1) {
            let l = labels[y as usize * w + x as usize];
            if l != 0 && g.ids.contains(&(l - 1)) {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
                area += 1;
            }
        }
    }
    (area >= 2).then(|| Piece { x0, y0, x1, y1, area, ids: g.ids.clone() })
}

fn confidence(d: f32, second: f32) -> f32 {
    (1.0 - d / 3.0).clamp(0.0, 1.0) * if second < f32::MAX { ((second - d) / (d + 0.3)).clamp(0.2, 1.0) } else { 1.0 }
}

const GOOD: f32 = 0.30;
const SPLIT_COST: f32 = 0.22;

fn one(m: &Model, ink: &[u8], labels: &[u32], w: usize, g: &Piece, baseline: f32, tall: f32) -> (Read, f32) {
    let f = features(ink, labels, w, g, baseline, tall);
    let (ch, d, second) = m.nearest(&f);
    (Read { ch, conf: confidence(d, second), x0: g.x0, x1: g.x1, y0: g.y0, y1: g.y1 }, d)
}

/// Reads one glyph, or, when it matches nothing well, tries cutting it at the thin places of its columns (letters that touch:
/// `rn` against `m`, `ti`, `ff`) and keeps the cut when the parts match better than the whole.
fn classify(m: &Model, ink: &[u8], labels: &[u32], w: usize, g: &Piece, baseline: f32, tall: f32) -> Vec<Read> {
    split(m, ink, labels, w, g, baseline, tall, 2).0
}

#[allow(clippy::too_many_arguments)]
fn split(m: &Model, ink: &[u8], labels: &[u32], w: usize, g: &Piece, baseline: f32, tall: f32, depth: u32) -> (Vec<Read>, f32) {
    let (whole, d) = one(m, ink, labels, w, g, baseline, tall);
    if d <= GOOD || depth == 0 || (g.w() as f32) < tall * 0.75 {
        return (vec![whole], d);
    }
    // ink per column, thin places first
    let minw = ((tall * 0.16) as i32).max(2);
    let mut cols: Vec<(u32, i32)> = vec![];
    for x in g.x0 + minw..=g.x1 - minw {
        let mut n = 0u32;
        for y in g.y0..=g.y1 {
            let l = labels[y as usize * w + x as usize];
            if l != 0 && g.ids.contains(&(l - 1)) {
                n += 1;
            }
        }
        cols.push((n, x));
    }
    let local_min: Vec<(u32, i32)> = (0..cols.len()).filter(|&i| (i == 0 || cols[i].0 <= cols[i - 1].0) && (i + 1 == cols.len() || cols[i].0 <= cols[i + 1].0)).map(|i| cols[i]).collect();
    let mut cand = local_min;
    cand.sort();
    cand.truncate(5);
    let mut best: Option<(f32, Vec<Read>)> = None;
    for (_, x) in cand {
        let (Some(a), Some(b)) = (sub_piece(labels, w, g, g.x0, x - 1), sub_piece(labels, w, g, x + 1, g.x1)) else { continue };
        if a.w() < 2 || b.w() < 2 {
            continue;
        }
        let (mut ra, da) = split(m, ink, labels, w, &a, baseline, tall, depth - 1);
        let (rb, db) = split(m, ink, labels, w, &b, baseline, tall, depth - 1);
        let (wa, wb) = (a.w() as f32, b.w() as f32);
        let score = (da * wa + db * wb) / (wa + wb) + SPLIT_COST;
        if best.as_ref().is_none_or(|(s, _)| score < *s) {
            ra.extend(rb);
            best = Some((score, ra));
        }
    }
    match best {
        Some((s, parts)) if s < d * 0.8 => (parts, s),
        _ => (vec![whole], d),
    }
}

struct Raw {
    glyphs: Vec<Piece>,
    baseline: f32,
    tall: f32,
}

fn percentile(v: &mut [f32], q: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // nearest rank, rounding up, so that one tall letter among a few small ones still counts
    v[(((v.len() - 1) as f32 * q).ceil() as usize).min(v.len() - 1)]
}

/// Reads a page picture.
pub fn recognise(img: &Gray) -> OcrPage {
    let mut page = OcrPage { lines: vec![], w: img.w, h: img.h, skew: 0.0, scale: 0.0 };
    if img.w < 8 || img.h < 8 || (img.w as u64) * (img.h as u64) > MAX_PIXELS || model().t.is_empty() {
        return page;
    }
    // straighten, then read
    let skew = estimate_skew(img);
    let turned;
    let src = if skew.abs() >= 0.2 {
        turned = rotate(img, -skew);
        &turned
    } else {
        img
    };
    page.skew = if skew.abs() >= 0.2 { skew } else { 0.0 };
    read(src, &mut page);
    page
}

fn ink_of(img: &Gray) -> (Vec<u8>, Vec<bool>) {
    let thr = otsu(&img.px);
    // the darker side is the ink: white-on-black pages are turned over
    let dark = img.px.iter().filter(|&&p| p <= thr).count();
    let invert = dark * 2 > img.px.len();
    let v: Vec<u8> = if invert { img.px.iter().map(|p| 255 - p).collect() } else { img.px.clone() };
    let thr = if invert { 255 - thr } else { thr };
    let mut hist = [0u64; 256];
    for &p in &v {
        hist[p as usize] += 1;
    }
    let total = v.len() as u64;
    // the darkest tenth of a percent is full ink, the median is the paper
    let (mut dark_lvl, mut paper_lvl) = (0u32, 255u32);
    let mut a = 0;
    for (i, c) in hist.iter().enumerate() {
        a += c;
        if a * 1000 >= total {
            dark_lvl = i as u32;
            break;
        }
    }
    a = 0;
    for (i, c) in hist.iter().enumerate() {
        a += c;
        if a * 2 >= total {
            paper_lvl = i as u32;
            break;
        }
    }
    let span = paper_lvl.saturating_sub(dark_lvl).max(1);
    let ink = v.iter().map(|&p| (paper_lvl.saturating_sub((p as u32).min(paper_lvl)) * 255 / span).min(255) as u8).collect();
    let mask = v.iter().map(|&p| p <= thr).collect();
    (ink, mask)
}

fn read(img: &Gray, page: &mut OcrPage) {
    let (w, h) = (img.w, img.h);
    let (ink, mask) = ink_of(img);
    let mut labels = Vec::new();
    let Some(mut ps) = pieces(&mask, w, h, &mut labels) else { return };
    // specks and page-sized rules are not letters
    ps.retain(|p| p.area >= 3 && (p.w() as usize) < w / 2 && (p.h() as usize) < h / 3 && p.w() < 3 * p.h().max(8) * 6);
    if ps.is_empty() {
        return;
    }
    // lines: rows covered by the boxes of pieces; a tall band is cut where the ink is thinnest
    let mut cover = vec![0i32; h + 1];
    for p in &ps {
        cover[p.y0 as usize] += 1;
        cover[p.y1 as usize + 1] -= 1;
    }
    let mut bands: Vec<(i32, i32)> = vec![];
    let (mut run, mut start) = (0, 0i32);
    for (y, c) in cover.iter().enumerate().take(h) {
        let was = run > 0;
        run += c;
        if run > 0 && !was {
            start = y as i32;
        } else if run <= 0 && was {
            bands.push((start, y as i32 - 1));
        }
    }
    if run > 0 {
        bands.push((start, h as i32 - 1));
    }
    let mut hs: Vec<f32> = bands.iter().map(|b| (b.1 - b.0 + 1) as f32).collect();
    let med_band = percentile(&mut hs, 0.5).max(4.0);
    let mut rowink = vec![0u32; h];
    for p in &ps {
        for y in p.y0..=p.y1 {
            rowink[y as usize] += p.w() as u32;
        }
    }
    let mut cuts: Vec<(i32, i32)> = vec![];
    for (a, b) in bands {
        let bh = (b - a + 1) as f32;
        let k = (bh / med_band).round() as i32;
        if k >= 2 && bh > med_band * 1.9 && k <= 12 {
            let mut from = a;
            for i in 1..k {
                let expect = a + (bh * i as f32 / k as f32) as i32;
                let r = (med_band * 0.3) as i32;
                let cut = (expect - r..=expect + r).filter(|y| *y > from && *y < b).min_by_key(|y| rowink[*y as usize]).unwrap_or(expect);
                cuts.push((from, cut - 1));
                from = cut;
            }
            cuts.push((from, b));
        } else {
            cuts.push((a, b));
        }
    }
    let mut raws: Vec<Raw> = vec![];
    for (a, b) in &cuts {
        let mine: Vec<Piece> = ps.iter().filter(|p| {
            let c = (p.y0 + p.y1) / 2;
            c >= *a && c <= *b
        }).cloned().collect();
        if mine.is_empty() {
            continue;
        }
        let glyphs = join_stacked(mine, ((b - a + 1) as f32 * 0.6) as i32);
        let mut bottoms: Vec<f32> = glyphs.iter().map(|g| (g.y1 + 1) as f32).collect();
        let baseline = percentile(&mut bottoms, 0.6);
        let mut tops: Vec<f32> = glyphs.iter().filter(|g| (g.y1 + 1) as f32 <= baseline + 1.5).map(|g| baseline - g.y0 as f32).collect();
        let tall = percentile(&mut tops, 0.9);
        raws.push(Raw { glyphs, baseline, tall });
    }
    if raws.is_empty() {
        return;
    }
    let mut talls: Vec<f32> = raws.iter().filter(|r| r.glyphs.len() >= 5).map(|r| r.tall).collect();
    if talls.is_empty() {
        talls = raws.iter().map(|r| r.tall).collect();
    }
    let page_tall = percentile(&mut talls, 0.5);
    let m = model();
    for r in &raws {
        // a line of a few letters may have no tall ones: it borrows from the page; a line of its own size is trusted (headings)
        let tall = if r.glyphs.len() >= 6 { r.tall } else { r.tall.max(page_tall * 0.85) }.max(3.0);
        let mut words: Vec<OcrWord> = vec![];
        let mut cur: Option<(String, Piece, f32, usize)> = None;
        let mut prev_x1: Option<i32> = None;
        // a gap is a space when it is wide against the gaps of this line (a fixed width font has wide gaps between its letters)
        let mut gaps: Vec<f32> = r.glyphs.windows(2).map(|p| (p[1].x0 - p[0].x1 - 1).max(0) as f32).collect();
        let word_gap = (0.30 * tall).max(1.9 * percentile(&mut gaps, 0.5));
        for g in &r.glyphs {
            let parts = classify(m, &ink, &labels, w, g, r.baseline, tall);
            // a glyph that was split is several letters side by side: all but the last are added to the word here
            let (last, firsts) = parts.split_last().map(|(l, f)| (l.clone(), f.to_vec())).unwrap_or_else(|| (Read { ch: '?', conf: 0.0, x0: g.x0, x1: g.x1, y0: g.y0, y1: g.y1 }, vec![]));
            for p in firsts {
                let gap = prev_x1.map(|x| p.x0 - x - 1).unwrap_or(0);
                if prev_x1.is_some() && gap as f32 > word_gap {
                    if let Some((t, b, c, n)) = cur.take() {
                        words.push(OcrWord { text: t, x0: b.x0, y0: b.y0, x1: b.x1 + 1, y1: b.y1 + 1, confidence: c / n as f32 });
                    }
                }
                let pc = Piece { x0: p.x0, y0: p.y0, x1: p.x1, y1: p.y1, area: 0, ids: vec![] };
                match cur.as_mut() {
                    Some((t, b, c, n)) => {
                        t.push(p.ch);
                        b.x0 = b.x0.min(pc.x0);
                        b.x1 = b.x1.max(pc.x1);
                        b.y0 = b.y0.min(pc.y0);
                        b.y1 = b.y1.max(pc.y1);
                        *c += p.conf;
                        *n += 1;
                    }
                    None => cur = Some((p.ch.to_string(), pc, p.conf, 1)),
                }
                prev_x1 = Some(p.x1);
            }
            let (ch, conf) = (last.ch, last.conf);
            let g = &Piece { x0: last.x0, y0: last.y0, x1: last.x1, y1: last.y1, area: 0, ids: vec![] };
            let gap = prev_x1.map(|x| g.x0 - x - 1).unwrap_or(0);
            let new_word = prev_x1.is_some() && gap as f32 > word_gap;
            if new_word {
                if let Some((t, b, c, n)) = cur.take() {
                    words.push(OcrWord { text: t, x0: b.x0, y0: b.y0, x1: b.x1 + 1, y1: b.y1 + 1, confidence: c / n as f32 });
                }
            }
            match cur.as_mut() {
                Some((t, b, c, n)) => {
                    t.push(ch);
                    b.x0 = b.x0.min(g.x0);
                    b.x1 = b.x1.max(g.x1);
                    b.y0 = b.y0.min(g.y0);
                    b.y1 = b.y1.max(g.y1);
                    *c += conf;
                    *n += 1;
                }
                None => cur = Some((ch.to_string(), g.clone(), conf, 1)),
            }
            prev_x1 = Some(g.x1);
        }
        if let Some((t, b, c, n)) = cur.take() {
            words.push(OcrWord { text: t, x0: b.x0, y0: b.y0, x1: b.x1 + 1, y1: b.y1 + 1, confidence: c / n as f32 });
        }
        let mut start = true;
        for wd in words.iter_mut() {
            wd.text = repair_in(&wd.text, start);
            start = wd.text.ends_with(['.', '?', '!']);
        }
        words.retain(|w| !w.text.is_empty());
        if !words.is_empty() {
            page.lines.push(OcrLine { words, baseline: r.baseline, height: tall });
        }
    }
    page.lines.sort_by(|a, b| a.baseline.partial_cmp(&b.baseline).unwrap_or(std::cmp::Ordering::Equal));
}

/// Words that begin with a capital I (the shapes of `I` and `l` are the same in many fonts, so a word that begins a sentence is
/// a capital I only when it is one of these; elsewhere in a sentence it is `l`).
const I_WORDS: &[&str] = &[
    "I", "I'm", "I'll", "I've", "I'd", "In", "It", "Is", "If", "Its", "Into", "Inc", "Info", "Item", "Items", "Invoice", "Invoices", "Instructions", "International",
    "Internet", "Important", "Include", "Included", "Including", "Information", "Insurance", "Interest", "Income", "Issue", "Issues", "Instead", "Inside", "Indeed", "Individual",
    "Institute", "Introduction", "Index", "Image", "Images", "Ideas", "Idea", "Identity", "Immediately", "Invited", "Inspection", "Industry", "Intro", "Invalid",
];

pub fn repair(w: &str) -> String {
    repair_in(w, false)
}

/// Letters that look alike are decided by their neighbours: in a word of letters, `0` is `O`, `1`, `|` and `I` after a small letter
/// are `l`; in a word of digits `O` and `l` are `0` and `1`. `sentence_start` says the word begins a sentence.
pub fn repair_in(w: &str, sentence_start: bool) -> String {
    let c: Vec<char> = w.replace("''", "\"").chars().collect();
    let letters = c.iter().filter(|x| x.is_ascii_alphabetic()).count();
    let digits = c.iter().filter(|x| x.is_ascii_digit()).count();
    let lower_total = c.iter().filter(|x| x.is_ascii_lowercase() && **x != 'l').count();
    let mut out: Vec<char> = Vec::with_capacity(c.len());
    for (i, &ch) in c.iter().enumerate() {
        let lower_before = out.last().is_some_and(|x| x.is_ascii_lowercase());
        let lower_after = c.get(i + 1).is_some_and(|x| x.is_ascii_lowercase());
        let upper_before = out.last().is_some_and(|x| x.is_ascii_uppercase());
        let r = match ch {
            '0' if letters >= 2 && digits == 1 => if lower_before || lower_after { 'o' } else { 'O' },
            '1' if letters >= 2 && digits == 1 => 'l',
            '|' if letters >= 2 => 'l',
            'I' if lower_before => 'l',
            // letters whose capital has the shape of the small one: a capital in the middle of a word of small letters is the small one
            'C' | 'O' | 'S' | 'V' | 'W' | 'X' | 'Z' | 'U' | 'K' if lower_before && lower_total >= 2 => ch.to_ascii_lowercase(),
            'I' | 'l' if i == 0 && c.len() >= 2 && lower_after => {
                // the first letter of a word of small letters
                let as_cap: String = std::iter::once('I').chain(c[1..].iter().copied()).collect();
                let bare = as_cap.trim_end_matches(|x: char| !x.is_ascii_alphanumeric() && x != '\'');
                if sentence_start && I_WORDS.contains(&bare) { 'I' } else { 'l' }
            }
            'l' if upper_before && lower_total == 0 && letters >= 3 && !c.get(i + 1).is_some_and(|x| x.is_ascii_lowercase()) => 'I',
            'O' if digits >= 2 && letters == 1 => '0',
            'l' if digits >= 2 && letters == 1 => '1',
            x => x,
        };
        out.push(r);
    }
    out.into_iter().collect()
}

// ------------------------------------------------------------------------------------------------ skew

fn ink_rows_sq(mask: &[bool], w: usize, h: usize, slope: f32, step: usize) -> f64 {
    // the sharpness of the row profile after shearing y' = y + slope * (x - w/2), rows binned `step` at a time so that sampling
    // every `step`th pixel does not make a comb of the profile
    let reach = (w as f32 * slope.abs() / 2.0) as i32 + 2;
    let mut rows = vec![0u32; h / step + 2 * (reach as usize / step + 2) + 4];
    let off = (reach as usize / step + 2) as i32;
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            if mask[y * w + x] {
                let yy = y as f32 + slope * (x as f32 - w as f32 / 2.0);
                let i = ((yy / step as f32).floor() as i32 + off).clamp(0, rows.len() as i32 - 1) as usize;
                rows[i] += 1;
            }
        }
    }
    rows.windows(2).map(|p| (p[0] as f64 - p[1] as f64).powi(2)).sum()
}

/// Degrees (positive: the text runs downwards to the right) found by the sharpest row profile, within 3 degrees.
pub fn estimate_skew(img: &Gray) -> f32 {
    if img.w < 64 || img.h < 64 {
        return 0.0;
    }
    let thr = otsu(&img.px);
    let dark = img.px.iter().filter(|&&p| p <= thr).count();
    let invert = dark * 2 > img.px.len();
    let mask: Vec<bool> = img.px.iter().map(|&p| if invert { p > thr } else { p <= thr }).collect();
    let step = ((img.w.max(img.h)) / 1100).max(1);
    let mut best = (f64::MIN, 0.0f32);
    let mut a = -3.0f32;
    let base = ink_rows_sq(&mask, img.w, img.h, 0.0, step);
    while a <= 3.001 {
        let s = ink_rows_sq(&mask, img.w, img.h, a.to_radians().tan(), step);
        if s > best.0 {
            best = (s, a);
        }
        a += 0.25;
    }
    // a closer look around the best angle
    let centre = best.1;
    let mut a = centre - 0.2;
    while a <= centre + 0.201 {
        let s = ink_rows_sq(&mask, img.w, img.h, a.to_radians().tan(), step);
        if s > best.0 {
            best = (s, a);
        }
        a += 0.05;
    }
    // only turn when it clearly helps
    if best.0 > base * 1.05 { best.1 } else { 0.0 }
}

/// Turns the picture so that text running `deg` downwards to the right becomes level (bilinear, white fill).
pub fn rotate(img: &Gray, deg: f32) -> Gray {
    let (w, h) = (img.w as f32, img.h as f32);
    let (s, c) = ((-deg).to_radians().sin(), (-deg).to_radians().cos());
    let (cx, cy) = (w / 2.0, h / 2.0);
    let mut px = vec![255u8; img.px.len()];
    for y in 0..img.h {
        for x in 0..img.w {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            let (sx, sy) = (cx + dx * c + dy * s, cy - dx * s + dy * c);
            if sx < 0.0 || sy < 0.0 || sx >= w - 1.0 || sy >= h - 1.0 {
                continue;
            }
            let (x0, y0) = (sx as usize, sy as usize);
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let g = |xx: usize, yy: usize| img.px[yy * img.w + xx] as f32;
            let v = g(x0, y0) * (1.0 - fx) * (1.0 - fy) + g(x0 + 1, y0) * fx * (1.0 - fy) + g(x0, y0 + 1) * (1.0 - fx) * fy + g(x0 + 1, y0 + 1) * fx * fy;
            px[y * img.w + x] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    Gray { w: img.w, h: img.h, px }
}

#[cfg(test)]
#[path = "ocr_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "ocr_train_tests.rs"]
mod train;
