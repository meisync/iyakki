//! A QR code reader of our own, the counterpart of [`crate::qr`] (ISO/IEC 18004 is a public standard): a camera picture in,
//! the bytes of the code out. It exists so the sign-in dialog can read the code a person shows on a phone screen to the camera.
//! No dependency, no unsafe code, and everything is bounded: a frame larger than [`MAX_PIXELS`] is refused, the work is a fixed
//! number of passes over the pixels plus a capped number of candidate symbols, and a time budget (about 30 ms at 640x480) stops
//! the search for further candidates.
//!
//! What it reads: versions 1 to [`MAX_VERSION`] (21x21 to 57x57 modules), levels L, M, Q and H with Reed-Solomon correction (up
//! to half the error-correction codewords of a block), all eight masks, numeric, alphanumeric and byte segments and ECI
//! markers (skipped; the bytes are returned as they are). Pictures: any size up to the limit, uneven light (a local-mean
//! threshold, then a global one), any rotation and a moderate tilt (the four-point mapping uses the alignment pattern of the
//! symbol when it finds one), any scale from about 3 pixels per module up, and a mirrored picture (a front camera) is tried too.
//!
//! What it does not read: versions above 10, Kanji, structured append, FNC1, Micro QR, several symbols in one picture (the
//! first one that checks wins), strong lens distortion, a symbol cut by the edge of the picture, and an inverted (light on dark)
//! code. A symbol the Reed-Solomon check cannot repair gives `None`, never a guess.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::qr::{self, Grid};

/// Largest frame (pixels) that is looked at; a bigger one is refused at once (shrink it first).
pub const MAX_PIXELS: usize = 2_500_000;
/// Largest version read.
pub const MAX_VERSION: usize = 10;
/// Most bytes a result may have (the largest symbol read holds fewer).
const MAX_OUT: usize = 3000;
/// Finder-pattern candidates kept, and the best of them tried in triples.
const MAX_CANDS: usize = 64;
const TRY_CANDS: usize = 12;

/// The time allowed for one frame: 30 ms up to 640x480, in proportion above that, at most 150 ms. The first candidate symbol is
/// always tried to the end; the budget stops the search for more.
pub fn default_budget(w: usize, h: usize) -> Duration {
    let ms = (30 * w.saturating_mul(h).max(1) / (640 * 480)).clamp(30, 150);
    Duration::from_millis(ms as u64)
}

/// Read a QR code in a grayscale picture (`w * h` bytes, row by row, 0 dark). `None` when there is none, it is damaged beyond
/// repair, or the frame is empty, short or larger than [`MAX_PIXELS`].
pub fn decode_gray(w: usize, h: usize, gray: &[u8]) -> Option<Vec<u8>> {
    if !frame_ok(w, h) {
        return None;
    }
    decode_with(w, h, gray, default_budget(w, h))
}

/// The same for an RGB picture (3 bytes a pixel).
pub fn decode_rgb(w: usize, h: usize, rgb: &[u8]) -> Option<Vec<u8>> {
    decode_packed(w, h, rgb, 3)
}

/// The same for an RGBA picture (4 bytes a pixel, alpha ignored).
pub fn decode_rgba(w: usize, h: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    decode_packed(w, h, rgba, 4)
}

fn decode_packed(w: usize, h: usize, px: &[u8], bpp: usize) -> Option<Vec<u8>> {
    if !frame_ok(w, h) || px.len() < w * h * bpp {
        return None;
    }
    let gray: Vec<u8> = px.chunks_exact(bpp).take(w * h).map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8).collect();
    decode_gray(w, h, &gray)
}

fn frame_ok(w: usize, h: usize) -> bool {
    w >= 21 && h >= 21 && w.checked_mul(h).is_some_and(|n| n <= MAX_PIXELS)
}

/// [`decode_gray`] with an explicit time budget (the tests give a long one so a slow debug build is not cut short).
pub fn decode_with(w: usize, h: usize, gray: &[u8], budget: Duration) -> Option<Vec<u8>> {
    if !frame_ok(w, h) || gray.len() < w * h {
        return None;
    }
    let end = Instant::now() + budget;
    let gray = &gray[..w * h];
    let mut first = true;
    for kind in [Thresh::Local, Thresh::Global] {
        let bin = binarize(w, h, gray, kind);
        let cands = find_finders(&bin);
        if cands.len() < 3 {
            continue;
        }
        // the picture and its mirror image take turns, best candidate first, so a mirrored symbol is not left to the end of the budget
        let mirrored: Vec<Cand> = cands.iter().map(|c| Cand { x: w as f64 - c.x, ..*c }).collect();
        let (a, b) = (triples(&cands), triples(&mirrored));
        let order = (0..a.len().max(b.len())).flat_map(|i| [(false, a.get(i)), (true, b.get(i))]);
        for (mirror, t) in order {
            let Some(t) = t else { continue };
            if !first && Instant::now() >= end {
                return None;
            }
            first = false;
            if let Some(d) = try_triple(&View { bin: &bin, mirror }, t) {
                return Some(d);
            }
        }
    }
    None
}

// ------------------------------------------------------------------------------------------------ black and white

#[derive(Clone, Copy)]
enum Thresh {
    /// Each pixel against the mean of its surroundings: uneven light.
    Local,
    /// One level for the whole picture (Otsu): a clean picture with a big symbol.
    Global,
}

struct Bin {
    w: usize,
    h: usize,
    /// `true` = dark.
    d: Vec<bool>,
}

fn binarize(w: usize, h: usize, g: &[u8], kind: Thresh) -> Bin {
    let mut d = vec![false; w * h];
    match kind {
        Thresh::Global => {
            let mut hist = [0u32; 256];
            for &v in g {
                hist[v as usize] += 1;
            }
            let total = (w * h) as f64;
            let sum_all: f64 = hist.iter().enumerate().map(|(i, &c)| i as f64 * c as f64).sum();
            let (mut wb, mut sum_b, mut best, mut level) = (0.0f64, 0.0f64, -1.0f64, 128u8);
            for (t, &c) in hist.iter().enumerate() {
                wb += c as f64;
                if wb == 0.0 {
                    continue;
                }
                let wf = total - wb;
                if wf == 0.0 {
                    break;
                }
                sum_b += t as f64 * c as f64;
                let (mb, mf) = (sum_b / wb, (sum_all - sum_b) / wf);
                let between = wb * wf * (mb - mf) * (mb - mf);
                if between > best {
                    best = between;
                    level = t as u8;
                }
            }
            for (o, &v) in d.iter_mut().zip(g) {
                *o = v <= level;
            }
        }
        Thresh::Local => {
            // an integral image gives the mean of a square window in four reads
            let stride = w + 1;
            let mut ii = vec![0u32; stride * (h + 1)];
            for y in 0..h {
                let mut row = 0u32;
                for x in 0..w {
                    row += g[y * w + x] as u32;
                    ii[(y + 1) * stride + x + 1] = ii[y * stride + x + 1] + row;
                }
            }
            let r = (w.min(h) / 8).max(8);
            for y in 0..h {
                let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
                for x in 0..w {
                    let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
                    let sum = ii[y1 * stride + x1] + ii[y0 * stride + x0] - ii[y0 * stride + x1] - ii[y1 * stride + x0];
                    let area = ((x1 - x0) * (y1 - y0)) as u32;
                    // dark when clearly under the local mean (a flat area stays light however noisy)
                    d[y * w + x] = g[y * w + x] as u32 * area * 100 < sum * 88;
                }
            }
        }
    }
    Bin { w, h, d }
}

/// The binary picture as the symbol sees it, optionally mirrored left to right.
struct View<'a> {
    bin: &'a Bin,
    mirror: bool,
}

impl View<'_> {
    /// Is the pixel at this position (pixel `i` covers `i..i+1`) dark? Outside the picture: light.
    fn dark(&self, x: f64, y: f64) -> bool {
        if x < 0.0 || y < 0.0 {
            return false;
        }
        let (mut px, py) = (x as usize, y as usize);
        if px >= self.bin.w || py >= self.bin.h {
            return false;
        }
        if self.mirror {
            px = self.bin.w - 1 - px;
        }
        self.bin.d[py * self.bin.w + px]
    }
}

// ------------------------------------------------------------------------------------------------ finder patterns

#[derive(Clone, Copy, Debug)]
struct Cand {
    x: f64,
    y: f64,
    /// Module size in pixels.
    m: f64,
    /// How many scan lines agreed.
    n: u32,
}

/// 1:1:3:1:1 within a tolerance: the module size, or `None`.
fn finder_ratio(c: &[f64; 5]) -> Option<f64> {
    if c.iter().any(|&v| v < 1.0) {
        return None;
    }
    let m = c.iter().sum::<f64>() / 7.0;
    let v = m * 0.55;
    ((c[0] - m).abs() < v && (c[1] - m).abs() < v && (c[2] - 3.0 * m).abs() < 3.0 * v && (c[3] - m).abs() < v && (c[4] - m).abs() < v).then_some(m)
}

/// The five runs (dark, light, dark, light, dark) through the dark pixel `(x, y)` along `(dx, dy)`, the middle one being the run
/// that holds the pixel. Returns the centre of the middle run along the axis and the five lengths; `None` when a run is cut by the
/// edge of the picture.
fn cross(b: &Bin, x: usize, y: usize, dx: isize, dy: isize) -> Option<(f64, [f64; 5])> {
    let at = |px: isize, py: isize| -> Option<bool> { (px >= 0 && py >= 0 && (px as usize) < b.w && (py as usize) < b.h).then(|| b.d[py as usize * b.w + px as usize]) };
    let mut c = [0f64; 5];
    let (mut px, mut py) = (x as isize, y as isize);
    let mut back = 0;
    for (slot, want) in [(2usize, true), (1, false), (0, true)] {
        while at(px, py)? == want {
            c[slot] += 1.0;
            if slot == 2 {
                back += 1;
            }
            px -= dx;
            py -= dy;
        }
    }
    let (mut px, mut py) = (x as isize + dx, y as isize + dy);
    for (slot, want) in [(2usize, true), (3, false), (4, true)] {
        while at(px, py)? == want {
            c[slot] += 1.0;
            px += dx;
            py += dy;
        }
    }
    let t = if dx != 0 { x } else { y } as f64;
    Some((t - (back - 1) as f64 + c[2] / 2.0, c))
}

fn find_finders(b: &Bin) -> Vec<Cand> {
    let mut found: Vec<Cand> = vec![];
    let mut runs: Vec<(usize, usize, bool)> = Vec::with_capacity(b.w.min(4096));
    for y in 0..b.h {
        runs.clear();
        let row = &b.d[y * b.w..(y + 1) * b.w];
        let mut start = 0;
        for x in 1..=b.w {
            if x == b.w || row[x] != row[start] {
                runs.push((start, x - start, row[start]));
                start = x;
            }
        }
        for i in 0..runs.len().saturating_sub(4) {
            if !runs[i].2 {
                continue;
            }
            let hc = [runs[i].1, runs[i + 1].1, runs[i + 2].1, runs[i + 3].1, runs[i + 4].1].map(|v| v as f64);
            let Some(hm) = finder_ratio(&hc) else { continue };
            let xi = runs[i + 2].0 + runs[i + 2].1 / 2;
            let Some((cy, vc)) = cross(b, xi, y, 0, 1) else { continue };
            let Some(vm) = finder_ratio(&vc) else { continue };
            let (ht, vt) = (hc.iter().sum::<f64>(), vc.iter().sum::<f64>());
            if (vt - ht).abs() * 5.0 >= ht * 2.0 {
                continue;
            }
            let Some((cx, hc2)) = cross(b, xi, (cy as usize).min(b.h - 1), 1, 0) else { continue };
            let Some(hm2) = finder_ratio(&hc2) else { continue };
            let m = (hm + vm + hm2) / 3.0;
            // the same finder seen on another line: average it in
            let near = found.iter().position(|c| (c.x - cx).hypot(c.y - cy) < 1.5 * m.max(c.m) && c.m / m < 1.6 && m / c.m < 1.6);
            match near {
                Some(i) => {
                    let c = &mut found[i];
                    let n = c.n as f64;
                    c.x = (c.x * n + cx) / (n + 1.0);
                    c.y = (c.y * n + cy) / (n + 1.0);
                    c.m = (c.m * n + m) / (n + 1.0);
                    c.n += 1;
                }
                None if found.len() < MAX_CANDS => found.push(Cand { x: cx, y: cy, m, n: 1 }),
                None => {}
            }
        }
    }
    // a real finder is seen on several lines; when too few are, take what there is
    let strong: Vec<Cand> = found.iter().copied().filter(|c| c.n >= 2).collect();
    let mut out = if strong.len() >= 3 { strong } else { found };
    out.sort_by(|a, b| b.n.cmp(&a.n));
    out.truncate(TRY_CANDS);
    out
}

/// Three finder centres in the order top-left, top-right, bottom-left, with the module size.
#[derive(Clone, Copy, Debug)]
struct Triple {
    tl: (f64, f64),
    tr: (f64, f64),
    bl: (f64, f64),
    m: f64,
    score: f64,
}

fn triples(c: &[Cand]) -> Vec<Triple> {
    let mut out = vec![];
    let d = |a: &Cand, b: &Cand| (a.x - b.x).hypot(a.y - b.y);
    for i in 0..c.len() {
        for j in i + 1..c.len() {
            for k in j + 1..c.len() {
                let p = [&c[i], &c[j], &c[k]];
                let ms = [p[0].m, p[1].m, p[2].m];
                let (mn, mx) = (ms.iter().cloned().fold(f64::MAX, f64::min), ms.iter().cloned().fold(0.0, f64::max));
                if mx > 1.7 * mn {
                    continue;
                }
                // the corner is the one opposite the longest side
                let sides = [d(p[1], p[2]), d(p[0], p[2]), d(p[0], p[1])];
                let tl = (0..3).max_by(|&a, &b| sides[a].total_cmp(&sides[b])).unwrap_or(0);
                let (a, b) = (p[(tl + 1) % 3], p[(tl + 2) % 3]);
                let t = p[tl];
                let (va, vb) = ((a.x - t.x, a.y - t.y), (b.x - t.x, b.y - t.y));
                let (la, lb) = (va.0.hypot(va.1), vb.0.hypot(vb.1));
                if la < 1.0 || lb < 1.0 {
                    continue;
                }
                let cos = (va.0 * vb.0 + va.1 * vb.1) / (la * lb);
                let ratio = la.max(lb) / la.min(lb);
                if cos.abs() > 0.45 || ratio > 1.8 {
                    continue;
                }
                let m = (ms[0] + ms[1] + ms[2]) / 3.0;
                let dim = (la + lb) / 2.0 / m + 7.0;
                if !(17.0..=17.0 + 4.0 * MAX_VERSION as f64 + 8.0).contains(&dim) {
                    continue;
                }
                // y points down: with the corner at the origin, the top-right one is the first when the turn is clockwise
                let (tr, bl) = if va.0 * vb.1 - va.1 * vb.0 > 0.0 { (a, b) } else { (b, a) };
                // a plain square symbol scores best
                let score = cos.abs() + (ratio - 1.0) + (mx / mn - 1.0) * 0.5;
                out.push(Triple { tl: (t.x, t.y), tr: (tr.x, tr.y), bl: (bl.x, bl.y), m, score });
            }
        }
    }
    out.sort_by(|a, b| a.score.total_cmp(&b.score));
    out
}

// ------------------------------------------------------------------------------------------------ geometry

/// A projective map from module coordinates to picture coordinates.
#[derive(Clone, Copy, Debug)]
struct Homography([f64; 8]);

impl Homography {
    fn map(&self, u: f64, v: f64) -> (f64, f64) {
        let h = &self.0;
        let w = h[6] * u + h[7] * v + 1.0;
        ((h[0] * u + h[1] * v + h[2]) / w, (h[3] * u + h[4] * v + h[5]) / w)
    }

    /// The map that takes the four module points to the four picture points; `None` when they are degenerate.
    fn from_points(src: [(f64, f64); 4], dst: [(f64, f64); 4]) -> Option<Homography> {
        let mut a = [[0f64; 9]; 8];
        for i in 0..4 {
            let ((u, v), (x, y)) = (src[i], dst[i]);
            a[2 * i] = [u, v, 1.0, 0.0, 0.0, 0.0, -u * x, -v * x, x];
            a[2 * i + 1] = [0.0, 0.0, 0.0, u, v, 1.0, -u * y, -v * y, y];
        }
        for col in 0..8 {
            let piv = (col..8).max_by(|&p, &q| a[p][col].abs().total_cmp(&a[q][col].abs()))?;
            if a[piv][col].abs() < 1e-9 {
                return None;
            }
            a.swap(col, piv);
            for row in 0..8 {
                if row != col {
                    let f = a[row][col] / a[col][col];
                    for k in col..9 {
                        a[row][k] -= f * a[col][k];
                    }
                }
            }
        }
        let mut h = [0f64; 8];
        for i in 0..8 {
            h[i] = a[i][8] / a[i][i];
        }
        h.iter().all(|v| v.is_finite()).then_some(Homography(h))
    }
}

/// The versions worth trying for a symbol of about `dim` modules: the nearest first, then its neighbours.
fn versions_for(dim: f64) -> Vec<usize> {
    let est = (((dim - 17.0) / 4.0).round() as i64).clamp(1, MAX_VERSION as i64) as usize;
    let mut v = vec![est];
    let reach = if est >= 6 { 2 } else { 1 };
    for d in 1..=reach {
        if est + d <= MAX_VERSION {
            v.push(est + d);
        }
        if est > d {
            v.push(est - d);
        }
    }
    v
}

fn try_triple(view: &View, t: &Triple) -> Option<Vec<u8>> {
    let len = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1);
    // runs measured along the picture's rows are longer by 1/cos of the turn: put that back
    let turn = (t.tr.1 - t.tl.1).atan2(t.tr.0 - t.tl.0);
    let m = t.m * turn.cos().abs().max(turn.sin().abs());
    let dim = (len(t.tl, t.tr) + len(t.tl, t.bl)) / 2.0 / m + 7.0;
    for ver in versions_for(dim) {
        let n = (ver * 4 + 17) as f64;
        let br = (t.tr.0 + t.bl.0 - t.tl.0, t.tr.1 + t.bl.1 - t.tl.1);
        let src = [(3.5, 3.5), (n - 3.5, 3.5), (3.5, n - 3.5), (n - 3.5, n - 3.5)];
        let Some(h0) = Homography::from_points(src, [t.tl, t.tr, t.bl, br]) else { continue };
        if let Some(d) = read_symbol(view, &h0, ver) {
            return Some(d);
        }
        if ver >= 2 {
            // a tilted symbol: the alignment pattern near the bottom-right corner gives the fourth point
            if let Some(a) = find_alignment(view, &h0, ver, t) {
                let src = [(3.5, 3.5), (n - 3.5, 3.5), (3.5, n - 3.5), (n - 6.5, n - 6.5)];
                if let Some(h1) = Homography::from_points(src, [t.tl, t.tr, t.bl, a]) {
                    if let Some(d) = read_symbol(view, &h1, ver) {
                        return Some(d);
                    }
                }
            }
        }
    }
    None
}

/// Find the alignment pattern (a dark point in a light ring in a dark ring, 5x5 modules) near where `h0` puts it.
fn find_alignment(view: &View, h0: &Homography, ver: usize, t: &Triple) -> Option<(f64, f64)> {
    let n = (ver * 4 + 17) as f64;
    let (px, py) = h0.map(n - 6.5, n - 6.5);
    let ms = ((h0.map(1.0, 0.0).0 - h0.map(0.0, 0.0).0).hypot(h0.map(1.0, 0.0).1 - h0.map(0.0, 0.0).1)).max(1.0);
    // the axes of the symbol in the picture, one module long
    let ex = ((t.tr.0 - t.tl.0) / (n - 7.0), (t.tr.1 - t.tl.1) / (n - 7.0));
    let ey = ((t.bl.0 - t.tl.0) / (n - 7.0), (t.bl.1 - t.tl.1) / (n - 7.0));
    let step = (ms / 3.0).max(1.0);
    let reach = (3.5 * ms / step).ceil() as i32;
    let mut best = (0usize, 0.0f64, 0.0f64, 0u32);
    for iy in -reach..=reach {
        for ix in -reach..=reach {
            let (cx, cy) = (px + ix as f64 * step, py + iy as f64 * step);
            let mut score = 0;
            for j in -2..=2i32 {
                for i in -2..=2i32 {
                    let want = i.abs().max(j.abs()) != 1;
                    let (sx, sy) = (cx + i as f64 * ex.0 + j as f64 * ey.0, cy + i as f64 * ex.1 + j as f64 * ey.1);
                    score += usize::from(view.dark(sx, sy) == want);
                }
            }
            if score > best.0 {
                best = (score, cx, cy, 1);
            } else if score == best.0 && (cx - best.1 / best.3 as f64).hypot(cy - best.2 / best.3 as f64) < 2.0 * ms {
                best = (score, best.1 + cx, best.2 + cy, best.3 + 1);
            }
        }
    }
    (best.0 >= 22).then(|| (best.1 / best.3 as f64, best.2 / best.3 as f64))
}

// ------------------------------------------------------------------------------------------------ the symbol

/// Error-correction level in the order L, M, Q, H.
const ECC_PER_BLOCK: [[usize; MAX_VERSION]; 4] = [
    [7, 10, 15, 20, 26, 18, 20, 24, 30, 18],
    [10, 16, 26, 18, 24, 16, 18, 22, 22, 26],
    [13, 22, 18, 26, 18, 24, 18, 22, 20, 24],
    [17, 28, 22, 16, 22, 28, 26, 26, 24, 28],
];
const BLOCKS: [[usize; MAX_VERSION]; 4] = [[1, 1, 1, 1, 1, 2, 2, 2, 2, 4], [1, 1, 1, 2, 2, 4, 4, 4, 5, 5], [1, 1, 2, 2, 4, 4, 6, 6, 8, 8], [1, 1, 2, 4, 4, 4, 5, 6, 8, 8]];

/// The data codewords of a version at a level (also used by the tests that build symbols).
pub(crate) fn data_codewords(ver: usize, level: usize) -> usize {
    qr::raw_modules(ver) / 8 - ECC_PER_BLOCK[level][ver - 1] * BLOCKS[level][ver - 1]
}

/// The 15 format bits of 5 data bits (level and mask), with the mask the standard XORs in.
pub(crate) fn format_bits(data: usize) -> usize {
    let mut rem = data;
    for _ in 0..10 {
        rem = (rem << 1) ^ ((rem >> 9) * 0x537);
    }
    ((data << 10) | rem) ^ 0x5412
}

/// Level index (L, M, Q, H) and mask of the closest valid format word, when at most three bits differ.
fn match_format(raw: usize) -> Option<(usize, usize, u32)> {
    let (d, code) = (0..32usize).map(|d| (d, format_bits(d))).min_by_key(|&(_, c)| (c ^ raw).count_ones())?;
    let dist = (code ^ raw).count_ones();
    // the two level bits as written: 01 L, 00 M, 11 Q, 10 H
    let level = match d >> 3 {
        1 => 0,
        0 => 1,
        3 => 2,
        _ => 3,
    };
    (dist <= 3).then_some((level, d & 7, dist))
}

fn read_symbol(view: &View, h: &Homography, ver: usize) -> Option<Vec<u8>> {
    let s = ver * 4 + 17;
    // sample a module: the centre and eight points around it, the majority wins
    let at = |x: usize, y: usize| -> bool {
        let mut dark = 0;
        for oy in [-0.25, 0.0, 0.25] {
            for ox in [-0.25, 0.0, 0.25] {
                let (px, py) = h.map(x as f64 + 0.5 + ox, y as f64 + 0.5 + oy);
                dark += usize::from(view.dark(px, py));
            }
        }
        dark >= 5
    };
    // the format bits first: a wrong guess (version, corner, mirror) is turned down after 30 modules, not 3000
    let q = |x: usize, y: usize| at(x, y);
    let mut f1 = 0usize;
    for i in 0..=5 {
        f1 |= usize::from(q(8, i)) << i;
    }
    f1 |= usize::from(q(8, 7)) << 6 | usize::from(q(8, 8)) << 7 | usize::from(q(7, 8)) << 8;
    for i in 9..15 {
        f1 |= usize::from(q(14 - i, 8)) << i;
    }
    let mut f2 = 0usize;
    for i in 0..8 {
        f2 |= usize::from(q(s - 1 - i, 8)) << i;
    }
    for i in 8..15 {
        f2 |= usize::from(q(8, s - 15 + i)) << i;
    }
    let (level, mask, _) = [match_format(f1), match_format(f2)].into_iter().flatten().min_by_key(|x| x.2)?;
    let mut m = vec![false; s * s];
    for y in 0..s {
        for x in 0..s {
            m[y * s + x] = at(x, y);
        }
    }
    let q = |x: usize, y: usize| m[y * s + x];
    // which modules carry codewords: an empty symbol of this version, drawn as the encoder draws it
    let mut g = Grid { size: s, m: vec![false; s * s], f: vec![false; s * s] };
    for i in 0..s {
        g.set_fn(6, i, false);
        g.set_fn(i, 6, false);
    }
    for (cx, cy) in [(3, 3), (s as i32 - 4, 3), (3, s as i32 - 4)] {
        g.finder(cx, cy);
    }
    let pos = qr::align_positions(ver);
    let last = pos.len().saturating_sub(1);
    for (i, &cx) in pos.iter().enumerate() {
        for (j, &cy) in pos.iter().enumerate() {
            if !((i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0)) {
                g.alignment(cx, cy);
            }
        }
    }
    g.format_level(0, 0);
    g.version_info(ver);
    let total = qr::raw_modules(ver) / 8;
    let mut words = vec![0u8; total];
    let mut i = 0usize;
    let mut right = s as i32 - 1;
    while right >= 1 {
        if right == 6 {
            right = 5;
        }
        for vert in 0..s {
            for j in 0..2 {
                let x = (right - j) as usize;
                let y = if ((right + 1) & 2) == 0 { s - 1 - vert } else { vert };
                if !g.f[y * s + x] && i < total * 8 {
                    let bit = q(x, y) ^ qr::masked(mask, x, y);
                    words[i >> 3] |= u8::from(bit) << (7 - (i & 7));
                    i += 1;
                }
            }
        }
        right -= 2;
    }
    let (nb, ecl) = (BLOCKS[level][ver - 1], ECC_PER_BLOCK[level][ver - 1]);
    let (short, short_len) = (nb - total % nb, total / nb);
    let mut blocks: Vec<Vec<u8>> = vec![vec![]; nb];
    let mut k = 0;
    for col in 0..short_len + 1 {
        for (b, blk) in blocks.iter_mut().enumerate() {
            if col == short_len - ecl && b < short {
                continue;
            }
            blk.push(words[k]);
            k += 1;
        }
    }
    let mut data = vec![];
    for blk in blocks.iter_mut() {
        if !rs_correct(blk, ecl) {
            return None;
        }
        data.extend_from_slice(&blk[..blk.len() - ecl]);
    }
    parse_segments(&data, ver)
}

// ------------------------------------------------------------------------------------------------ Reed-Solomon

struct Gf {
    exp: [u8; 512],
    log: [u8; 256],
}

fn gf() -> &'static Gf {
    static G: OnceLock<Gf> = OnceLock::new();
    G.get_or_init(|| {
        let (mut exp, mut log) = ([0u8; 512], [0u8; 256]);
        let mut x = 1u16;
        for i in 0..255 {
            exp[i] = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= 0x11D;
            }
        }
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }
        Gf { exp, log }
    })
}

fn mul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let g = gf();
    g.exp[g.log[a as usize] as usize + g.log[b as usize] as usize]
}

fn div(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        return 0;
    }
    let g = gf();
    g.exp[g.log[a as usize] as usize + 255 - g.log[b as usize] as usize]
}

/// Evaluate a polynomial (lowest degree first) at `x`.
fn eval(p: &[u8], x: u8) -> u8 {
    p.iter().rev().fold(0, |acc, &c| mul(acc, x) ^ c)
}

/// Repair a block (data then `nsym` check codewords) in place: Berlekamp-Massey for the error locator, a search for its roots
/// and Forney's formula for the values. False when it cannot be repaired (more errors than `nsym / 2`, or the repair does not
/// check).
fn rs_correct(block: &mut [u8], nsym: usize) -> bool {
    let n = block.len();
    if n > 255 || nsym == 0 || nsym >= n {
        return false;
    }
    let g = gf();
    let syndromes = |b: &[u8]| -> Vec<u8> { (0..nsym).map(|j| b.iter().fold(0u8, |acc, &c| mul(acc, g.exp[j]) ^ c)).collect() };
    let s = syndromes(block);
    if s.iter().all(|&v| v == 0) {
        return true;
    }
    // the error locator
    let (mut c, mut b) = (vec![1u8], vec![1u8]);
    let (mut l, mut m, mut bb) = (0usize, 1usize, 1u8);
    for i in 0..nsym {
        let mut d = s[i];
        for j in 1..=l.min(c.len() - 1) {
            d ^= mul(c[j], s[i - j]);
        }
        if d == 0 {
            m += 1;
            continue;
        }
        let coef = div(d, bb);
        let old = c.clone();
        if c.len() < b.len() + m {
            c.resize(b.len() + m, 0);
        }
        for (j, &bj) in b.iter().enumerate() {
            c[j + m] ^= mul(coef, bj);
        }
        if 2 * l <= i {
            l = i + 1 - l;
            b = old;
            bb = d;
            m = 1;
        } else {
            m += 1;
        }
    }
    c.truncate(l + 1);
    if l == 0 || l > nsym / 2 || c.len() != l + 1 {
        return false;
    }
    // the error evaluator: S(x) * locator(x) mod x^l
    let omega: Vec<u8> = (0..l).map(|k| (0..=k).fold(0u8, |acc, i| acc ^ mul(s[i], c[k - i]))).collect();
    let mut fixed = 0;
    for p in 0..n {
        let xinv = g.exp[(255 - p % 255) % 255];
        if eval(&c, xinv) != 0 {
            continue;
        }
        // the formal derivative keeps the odd terms
        let deriv: Vec<u8> = (1..c.len()).map(|i| if i % 2 == 1 { c[i] } else { 0 }).collect();
        let dv = eval(&deriv, xinv);
        if dv == 0 {
            return false;
        }
        block[n - 1 - p] ^= mul(g.exp[p % 255], div(eval(&omega, xinv), dv));
        fixed += 1;
    }
    fixed == l && syndromes(block).iter().all(|&v| v == 0)
}

// ------------------------------------------------------------------------------------------------ the data

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn left(&self) -> usize {
        self.d.len() * 8 - self.pos
    }
    fn take(&mut self, n: usize) -> Option<usize> {
        if n > self.left() {
            return None;
        }
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | ((self.d[self.pos >> 3] >> (7 - (self.pos & 7))) & 1) as usize;
            self.pos += 1;
        }
        Some(v)
    }
}

const ALNUM: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

/// The bytes of the segments of a symbol; `None` for a mode this reader does not know.
pub(crate) fn parse_segments(data: &[u8], ver: usize) -> Option<Vec<u8>> {
    let mut r = Bits { d: data, pos: 0 };
    let mut out: Vec<u8> = vec![];
    let cc = |small: usize, mid: usize, big: usize| if ver <= 9 { small } else if ver <= 26 { mid } else { big };
    while r.left() >= 4 {
        let mode = r.take(4)?;
        match mode {
            0 => break,
            1 => {
                let mut n = r.take(cc(10, 12, 14))?;
                while n > 0 {
                    let (bits, k, max) = match n {
                        1 => (4, 1, 10),
                        2 => (7, 2, 100),
                        _ => (10, 3, 1000),
                    };
                    let v = r.take(bits)?;
                    if v >= max {
                        return None;
                    }
                    out.extend(format!("{v:0k$}").bytes());
                    n -= k;
                }
            }
            2 => {
                let mut n = r.take(cc(9, 11, 13))?;
                while n > 0 {
                    if n >= 2 {
                        let v = r.take(11)?;
                        if v >= 45 * 45 {
                            return None;
                        }
                        out.extend([ALNUM[v / 45], ALNUM[v % 45]]);
                        n -= 2;
                    } else {
                        let v = r.take(6)?;
                        out.push(*ALNUM.get(v)?);
                        n -= 1;
                    }
                }
            }
            4 => {
                let n = r.take(cc(8, 16, 16))?;
                for _ in 0..n {
                    out.push(r.take(8)? as u8);
                }
            }
            7 => {
                // an ECI designator of one to three bytes: skipped, the bytes are returned as they are
                let first = r.take(8)?;
                let more = if first & 0x80 == 0 { 0 } else if first & 0xC0 == 0x80 { 1 } else if first & 0xE0 == 0xC0 { 2 } else { return None };
                r.take(8 * more)?;
            }
            _ => return None,
        }
        if out.len() > MAX_OUT {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
#[path = "qr_decode_tests.rs"]
mod tests;
