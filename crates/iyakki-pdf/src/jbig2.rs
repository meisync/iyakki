//! JBIG2 decoding (ITU-T T.88) for the `JBIG2Decode` filter of scanned PDF pages (F-PDF-17), written in-house from the published
//! algorithms: no library, no downloaded code.
//!
//! What is read: the embedded stream organisation (segments one after the other, an optional global stream first), page information and
//! end of stripe, generic regions (arithmetic coding with templates 0 to 3, typical prediction and moving pixels, or MMR through the
//! fax decoder), generic refinement regions, symbol dictionaries (arithmetic coding, refinement and aggregation, exported symbols,
//! retained contexts) and text regions (arithmetic coding, all four corners, transposed, refinement), combined into the page with
//! OR, AND, XOR, XNOR and REPLACE.
//!
//! Halftone regions with their pattern dictionaries are read too (arithmetic coding; the grid may be rotated).
//!
//! Huffman coding is read too (`jbig2_huff.rs`): the standard tables B.1 to B.15, custom code tables, Huffman symbol dictionaries
//! (collective bitmaps raw or MMR, refinement and aggregation) and Huffman text regions.
//!
//! Halftone regions may be MMR coded (the bit planes one after the other) and may carry a skip bitmap.
//!
//! What is refused, with the reason in the error: colour extensions. A page that holds only a part of
//! these still gives the parts that were read; the rest of the page stays at its default value.

use std::collections::HashMap;
use std::rc::Rc;

use crate::ccitt;

#[path = "jbig2_huff.rs"]
mod huff;

/// One bit per byte: 1 is black, as in the standard.
#[derive(Clone, Debug, PartialEq)]
pub struct Bitmap {
    pub w: usize,
    pub h: usize,
    pub d: Vec<u8>,
}

/// The most pixels one bitmap may have (a page or a symbol), so a hostile size cannot take the memory.
const MAX_PIXELS: usize = 1 << 26;
/// The most pixels all symbols of one decode may have together.
const MAX_SYMBOL_PIXELS: usize = 1 << 27;

type R<T> = Result<T, String>;

impl Bitmap {
    pub fn new(w: usize, h: usize, v: u8) -> R<Bitmap> {
        if w.checked_mul(h).is_none_or(|n| n > MAX_PIXELS) {
            return Err(format!("bitmap of {w} by {h} is too large"));
        }
        Ok(Bitmap { w, h, d: vec![v; w * h] })
    }

    #[inline]
    pub fn get(&self, x: i64, y: i64) -> u8 {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            0
        } else {
            self.d[y as usize * self.w + x as usize]
        }
    }

    /// Draw `src` with its top left corner at (x, y) with a combination operator (0 OR, 1 AND, 2 XOR, 3 XNOR, 4 REPLACE).
    pub fn combine(&mut self, src: &Bitmap, x: i64, y: i64, op: u8) {
        for sy in 0..src.h as i64 {
            let dy = y + sy;
            if dy < 0 || dy >= self.h as i64 {
                continue;
            }
            for sx in 0..src.w as i64 {
                let dx = x + sx;
                if dx < 0 || dx >= self.w as i64 {
                    continue;
                }
                let s = src.d[sy as usize * src.w + sx as usize];
                let t = &mut self.d[dy as usize * self.w + dx as usize];
                *t = match op {
                    0 => *t | s,
                    1 => *t & s,
                    2 => *t ^ s,
                    3 => 1 ^ (*t ^ s),
                    _ => s,
                };
            }
        }
    }

    /// Grow to at least `h` rows, the new rows holding `v`.
    fn grow(&mut self, h: usize, v: u8) -> R<()> {
        if h > self.h {
            if self.w.checked_mul(h).is_none_or(|n| n > MAX_PIXELS) {
                return Err("page is too large".into());
            }
            self.d.resize(self.w * h, v);
            self.h = h;
        }
        Ok(())
    }

    fn sub(&self, x: i64, y: i64, w: usize, h: usize) -> R<Bitmap> {
        let mut b = Bitmap::new(w, h, 0)?;
        for sy in 0..h {
            for sx in 0..w {
                b.d[sy * w + sx] = self.get(x + sx as i64, y + sy as i64);
            }
        }
        Ok(b)
    }
}

// ------------------------------------------------------------------------------------------------ the arithmetic decoder (Annex E)

/// Qe, next state after a more probable symbol, next state after a less probable symbol, switch flag.
const QE: [(u16, u8, u8, u8); 47] = [
    (0x5601, 1, 1, 1),
    (0x3401, 2, 6, 0),
    (0x1801, 3, 9, 0),
    (0x0AC1, 4, 12, 0),
    (0x0521, 5, 29, 0),
    (0x0221, 38, 33, 0),
    (0x5601, 7, 6, 1),
    (0x5401, 8, 14, 0),
    (0x4801, 9, 14, 0),
    (0x3801, 10, 14, 0),
    (0x3001, 11, 17, 0),
    (0x2401, 12, 18, 0),
    (0x1C01, 13, 20, 0),
    (0x1601, 29, 21, 0),
    (0x5601, 15, 14, 1),
    (0x5401, 16, 14, 0),
    (0x5101, 17, 15, 0),
    (0x4801, 18, 16, 0),
    (0x3801, 19, 17, 0),
    (0x3401, 20, 18, 0),
    (0x3001, 21, 19, 0),
    (0x2801, 22, 19, 0),
    (0x2401, 23, 20, 0),
    (0x2201, 24, 21, 0),
    (0x1C01, 25, 22, 0),
    (0x1801, 26, 23, 0),
    (0x1601, 27, 24, 0),
    (0x1401, 28, 25, 0),
    (0x1201, 29, 26, 0),
    (0x1101, 30, 27, 0),
    (0x0AC1, 31, 28, 0),
    (0x09C1, 32, 29, 0),
    (0x08A1, 33, 30, 0),
    (0x0521, 34, 31, 0),
    (0x0441, 35, 32, 0),
    (0x02A1, 36, 33, 0),
    (0x0221, 37, 34, 0),
    (0x0141, 38, 35, 0),
    (0x0111, 39, 36, 0),
    (0x0085, 40, 37, 0),
    (0x0049, 41, 38, 0),
    (0x0025, 42, 39, 0),
    (0x0015, 43, 40, 0),
    (0x0009, 44, 41, 0),
    (0x0005, 45, 42, 0),
    (0x0001, 45, 43, 0),
    (0x5601, 46, 46, 0),
];

pub struct Mq<'a> {
    d: &'a [u8],
    bp: usize,
    chigh: u32,
    clow: u32,
    ct: i32,
    a: u32,
}

impl<'a> Mq<'a> {
    pub fn new(d: &'a [u8]) -> Mq<'a> {
        let mut m = Mq { d, bp: 0, chigh: d.first().copied().unwrap_or(0xFF) as u32, clow: 0, ct: 0, a: 0 };
        m.byte_in();
        m.chigh = ((m.chigh << 7) & 0xFFFF) | ((m.clow >> 9) & 0x7F);
        m.clow = (m.clow << 7) & 0xFFFF;
        m.ct -= 7;
        m.a = 0x8000;
        m
    }

    fn at(&self, i: usize) -> u32 {
        self.d.get(i).copied().unwrap_or(0xFF) as u32
    }

    fn byte_in(&mut self) {
        if self.at(self.bp) == 0xFF {
            if self.at(self.bp + 1) > 0x8F {
                self.clow += 0xFF00;
                self.ct = 8;
            } else {
                self.bp += 1;
                self.clow += self.at(self.bp) << 9;
                self.ct = 7;
            }
        } else {
            self.bp += 1;
            self.clow += if self.bp < self.d.len() { self.at(self.bp) << 8 } else { 0xFF00 };
            self.ct = 8;
        }
        if self.clow > 0xFFFF {
            self.chigh += self.clow >> 16;
            self.clow &= 0xFFFF;
        }
    }

    /// Decode one bit in context `cx[i]` (the state is the table index times two plus the more probable symbol).
    pub fn bit(&mut self, cx: &mut [u8], i: usize) -> u8 {
        let mut idx = (cx[i] >> 1) as usize;
        let mut mps = cx[i] & 1;
        let (qe, nmps, nlps, sw) = QE[idx];
        let qe = qe as u32;
        let mut a = self.a - qe;
        let d;
        if self.chigh < qe {
            // the less probable symbol's interval holds the code (exchange when the interval is the smaller)
            if a < qe {
                a = qe;
                d = mps;
                idx = nmps as usize;
            } else {
                a = qe;
                d = 1 ^ mps;
                if sw == 1 {
                    mps = d;
                }
                idx = nlps as usize;
            }
        } else {
            self.chigh -= qe;
            if a & 0x8000 != 0 {
                self.a = a;
                return mps;
            }
            if a < qe {
                d = 1 ^ mps;
                if sw == 1 {
                    mps = d;
                }
                idx = nlps as usize;
            } else {
                d = mps;
                idx = nmps as usize;
            }
        }
        loop {
            if self.ct == 0 {
                self.byte_in();
            }
            a <<= 1;
            self.chigh = ((self.chigh << 1) & 0xFFFF) | ((self.clow >> 15) & 1);
            self.clow = (self.clow << 1) & 0xFFFF;
            self.ct -= 1;
            if a & 0x8000 != 0 {
                break;
            }
        }
        self.a = a;
        cx[i] = ((idx as u8) << 1) | mps;
        d
    }
}

/// The integer decoding procedure of Annex A.2 with its 512 contexts; `None` is the out-of-band value.
pub struct IntCx(Vec<u8>);

impl IntCx {
    pub fn new() -> IntCx {
        IntCx(vec![0; 512])
    }

    pub fn decode(&mut self, mq: &mut Mq) -> Option<i64> {
        let mut prev = 1usize;
        let mut bit = |mq: &mut Mq, cx: &mut Vec<u8>| -> u32 {
            let b = mq.bit(cx, prev) as usize;
            prev = if prev < 256 { (prev << 1) | b } else { (((prev << 1) | b) & 511) | 256 };
            b as u32
        };
        let s = bit(mq, &mut self.0);
        let (n, off) = if bit(mq, &mut self.0) == 0 {
            (2, 0)
        } else if bit(mq, &mut self.0) == 0 {
            (4, 4)
        } else if bit(mq, &mut self.0) == 0 {
            (6, 20)
        } else if bit(mq, &mut self.0) == 0 {
            (8, 84)
        } else if bit(mq, &mut self.0) == 0 {
            (12, 340)
        } else {
            (32, 4436)
        };
        let mut v: i64 = 0;
        for _ in 0..n {
            v = (v << 1) | bit(mq, &mut self.0) as i64;
        }
        v += off;
        if s == 0 {
            Some(v)
        } else if v > 0 {
            Some(-v)
        } else {
            None
        }
    }
}

impl Default for IntCx {
    fn default() -> Self {
        IntCx::new()
    }
}

/// The symbol identifier decoding procedure of Annex A.3.
pub struct IdCx(Vec<u8>);

impl IdCx {
    pub fn new(len: u32) -> IdCx {
        IdCx(vec![0; 1usize << (len.min(30) + 1)])
    }

    pub fn decode(&mut self, mq: &mut Mq, len: u32) -> usize {
        let mut prev = 1usize;
        for _ in 0..len {
            let b = mq.bit(&mut self.0, prev) as usize;
            prev = (prev << 1) | b;
        }
        prev - (1usize << len)
    }
}

// ------------------------------------------------------------------------------------------------ generic region (6.2)

/// The size of the context table of a template.
fn generic_contexts(template: u8) -> usize {
    match template {
        0 => 1 << 16,
        1 => 1 << 13,
        _ => 1 << 10,
    }
}

/// The context that a typical prediction bit is decoded in (6.2.5.7).
fn sltp_context(template: u8) -> usize {
    match template {
        0 => 0x9B25,
        1 => 0x0795,
        2 => 0x00E5,
        _ => 0x0195,
    }
}

#[cfg(test)]
fn default_at(template: u8) -> Vec<(i8, i8)> {
    match template {
        0 => vec![(3, -1), (-3, -1), (2, -2), (-2, -2)],
        1 => vec![(3, -1)],
        _ => vec![(2, -1)],
    }
}

fn generic_context(bm: &Bitmap, x: i64, y: i64, template: u8, at: &[(i8, i8)]) -> usize {
    let p = |dx: i64, dy: i64| bm.get(x + dx, y + dy) as usize;
    let a = |i: usize| bm.get(x + at[i].0 as i64, y + at[i].1 as i64) as usize;
    match template {
        0 => {
            p(-1, 0)
                | p(-2, 0) << 1
                | p(-3, 0) << 2
                | p(-4, 0) << 3
                | a(0) << 4
                | p(2, -1) << 5
                | p(1, -1) << 6
                | p(0, -1) << 7
                | p(-1, -1) << 8
                | p(-2, -1) << 9
                | a(1) << 10
                | a(2) << 11
                | p(1, -2) << 12
                | p(0, -2) << 13
                | p(-1, -2) << 14
                | a(3) << 15
        }
        1 => {
            p(-1, 0)
                | p(-2, 0) << 1
                | p(-3, 0) << 2
                | a(0) << 3
                | p(2, -1) << 4
                | p(1, -1) << 5
                | p(0, -1) << 6
                | p(-1, -1) << 7
                | p(-2, -1) << 8
                | p(2, -2) << 9
                | p(1, -2) << 10
                | p(0, -2) << 11
                | p(-1, -2) << 12
        }
        2 => p(-1, 0) | p(-2, 0) << 1 | a(0) << 2 | p(1, -1) << 3 | p(0, -1) << 4 | p(-1, -1) << 5 | p(-2, -1) << 6 | p(1, -2) << 7 | p(0, -2) << 8 | p(-1, -2) << 9,
        _ => p(-1, 0) | p(-2, 0) << 1 | p(-3, 0) << 2 | p(-4, 0) << 3 | a(0) << 4 | p(1, -1) << 5 | p(0, -1) << 6 | p(-1, -1) << 7 | p(-2, -1) << 8 | p(-3, -1) << 9,
    }
}

/// Decode a `w` by `h` generic region with arithmetic coding. `cx` holds the context states (it may be kept for the next call).
pub fn decode_generic(mq: &mut Mq, cx: &mut [u8], w: usize, h: usize, template: u8, tpgdon: bool, at: &[(i8, i8)]) -> R<Bitmap> {
    decode_generic_skip(mq, cx, w, h, template, tpgdon, at, None)
}

/// As [`decode_generic`]; a pixel the `skip` bitmap marks is not decoded and is 0 (6.2.5.7, USESKIP).
#[allow(clippy::too_many_arguments)]
pub fn decode_generic_skip(mq: &mut Mq, cx: &mut [u8], w: usize, h: usize, template: u8, tpgdon: bool, at: &[(i8, i8)], skip: Option<&Bitmap>) -> R<Bitmap> {
    let mut bm = Bitmap::new(w, h, 0)?;
    let template = template.min(3);
    if cx.len() < generic_contexts(template) {
        return Err("context table too small".into());
    }
    let need = if template == 0 { 4 } else { 1 };
    if at.len() < need {
        return Err("adaptive template pixels missing".into());
    }
    let mut ltp = 0u8;
    for y in 0..h {
        if tpgdon {
            ltp ^= mq.bit(cx, sltp_context(template));
            if ltp == 1 {
                if y > 0 {
                    let (a, b) = bm.d.split_at_mut(y * w);
                    b[..w].copy_from_slice(&a[(y - 1) * w..y * w]);
                }
                continue;
            }
        }
        for x in 0..w {
            if skip.is_some_and(|s| s.get(x as i64, y as i64) == 1) {
                continue;
            }
            let c = generic_context(&bm, x as i64, y as i64, template, at);
            bm.d[y * w + x] = mq.bit(cx, c);
        }
    }
    Ok(bm)
}

// ------------------------------------------------------------------------------------------------ refinement region (6.3)

fn refinement_contexts(template: u8) -> usize {
    if template == 0 {
        1 << 13
    } else {
        1 << 10
    }
}

#[allow(clippy::too_many_arguments)]
pub fn decode_refinement(mq: &mut Mq, cx: &mut [u8], w: usize, h: usize, template: u8, reference: &Bitmap, dx: i64, dy: i64, tpgron: bool, at: &[(i8, i8)]) -> R<Bitmap> {
    let mut bm = Bitmap::new(w, h, 0)?;
    let template = template.min(1);
    if cx.len() < refinement_contexts(template) {
        return Err("context table too small".into());
    }
    if template == 0 && at.len() < 2 {
        return Err("adaptive template pixels missing".into());
    }
    let mut ltp = 0u8;
    for y in 0..h as i64 {
        if tpgron {
            // the context with only the reference's centre pixel set
            ltp ^= mq.bit(cx, if template == 0 { 0x100 } else { 0x80 });
        }
        for x in 0..w as i64 {
            let r = |i: i64, j: i64| reference.get(x - dx + i, y - dy + j) as usize;
            if ltp == 1 {
                let v = r(0, 0);
                if (-1..=1).all(|j| (-1..=1).all(|i| r(i, j) == v)) {
                    bm.d[y as usize * w + x as usize] = v as u8;
                    continue;
                }
            }
            let p = |i: i64, j: i64| bm.get(x + i, y + j) as usize;
            let c = if template == 0 {
                p(-1, 0)
                    | p(1, -1) << 1
                    | p(0, -1) << 2
                    | p(at[0].0 as i64, at[0].1 as i64) << 3
                    | r(1, 1) << 4
                    | r(0, 1) << 5
                    | r(-1, 1) << 6
                    | r(1, 0) << 7
                    | r(0, 0) << 8
                    | r(-1, 0) << 9
                    | r(1, -1) << 10
                    | r(0, -1) << 11
                    | r(at[1].0 as i64, at[1].1 as i64) << 12
            } else {
                p(-1, 0) | p(1, -1) << 1 | p(0, -1) << 2 | p(-1, -1) << 3 | r(1, 1) << 4 | r(0, 1) << 5 | r(1, 0) << 6 | r(0, 0) << 7 | r(-1, 0) << 8 | r(0, -1) << 9
            };
            bm.d[y as usize * w + x as usize] = mq.bit(cx, c);
        }
    }
    Ok(bm)
}

// ------------------------------------------------------------------------------------------------ text region (6.4)

struct TextParams {
    refine: bool,
    log_strips: u32,
    ref_corner: u8,
    transposed: bool,
    comb_op: u8,
    def_pixel: u8,
    ds_offset: i64,
    r_at: Vec<(i8, i8)>,
    num_instances: u32,
}

/// The integer contexts of a text region (they live on through the aggregate symbols of one symbol dictionary).
struct TextCx {
    dt: IntCx,
    fs: IntCx,
    ds: IntCx,
    it: IntCx,
    ri: IntCx,
    rdw: IntCx,
    rdh: IntCx,
    rdx: IntCx,
    rdy: IntCx,
    id: IdCx,
}

impl TextCx {
    fn new(code_len: u32) -> TextCx {
        TextCx { dt: IntCx::new(), fs: IntCx::new(), ds: IntCx::new(), it: IntCx::new(), ri: IntCx::new(), rdw: IntCx::new(), rdh: IntCx::new(), rdx: IntCx::new(), rdy: IntCx::new(), id: IdCx::new(code_len) }
    }
}

fn code_len(n: usize) -> u32 {
    let mut l = 0u32;
    while (1usize << l) < n {
        l += 1;
    }
    l
}

/// Where the values of a text region come from: the arithmetic decoder with its integer contexts, or Huffman tables (see `huff`).
trait TextSrc {
    fn dt(&mut self) -> R<i64>;
    fn first_s(&mut self) -> R<i64>;
    /// `None` ends a strip.
    fn next_s(&mut self) -> R<Option<i64>>;
    /// The T offset inside a strip (only asked when a strip is more than one row).
    fn curt(&mut self) -> R<i64>;
    fn id(&mut self) -> R<usize>;
    fn ri(&mut self) -> R<u8>;
    /// The symbol `base` refined as the data says.
    fn refined(&mut self, base: &Rc<Bitmap>, budget: &mut usize) -> R<Rc<Bitmap>>;
}

struct ArithText<'m, 'a> {
    mq: &'m mut Mq<'a>,
    tcx: &'m mut TextCx,
    gr_cx: &'m mut [u8],
    sym_code_len: u32,
    r_template: u8,
    r_at: &'m [(i8, i8)],
}

impl TextSrc for ArithText<'_, '_> {
    fn dt(&mut self) -> R<i64> {
        self.tcx.dt.decode(self.mq).ok_or_else(|| "text region: out-of-band strip".into())
    }
    fn first_s(&mut self) -> R<i64> {
        self.tcx.fs.decode(self.mq).ok_or_else(|| "text region: out-of-band first S".into())
    }
    fn next_s(&mut self) -> R<Option<i64>> {
        Ok(self.tcx.ds.decode(self.mq))
    }
    fn curt(&mut self) -> R<i64> {
        self.tcx.it.decode(self.mq).ok_or_else(|| "text region: out-of-band T".into())
    }
    fn id(&mut self) -> R<usize> {
        Ok(self.tcx.id.decode(self.mq, self.sym_code_len))
    }
    fn ri(&mut self) -> R<u8> {
        Ok(self.tcx.ri.decode(self.mq).ok_or("text region: out-of-band refinement flag")? as u8)
    }
    fn refined(&mut self, base: &Rc<Bitmap>, budget: &mut usize) -> R<Rc<Bitmap>> {
        let mq = &mut *self.mq;
        let rdw = self.tcx.rdw.decode(mq).ok_or("text region: out-of-band width")?;
        let rdh = self.tcx.rdh.decode(mq).ok_or("text region: out-of-band height")?;
        let rdx = self.tcx.rdx.decode(mq).ok_or("text region: out-of-band dx")?;
        let rdy = self.tcx.rdy.decode(mq).ok_or("text region: out-of-band dy")?;
        let nw = base.w as i64 + rdw;
        let nh = base.h as i64 + rdh;
        if nw < 0 || nh < 0 || nw > 1 << 16 || nh > 1 << 16 {
            return Err("text region: refined symbol size".into());
        }
        spend(budget, nw as usize * nh as usize)?;
        let gdx = rdw.div_euclid(2) + rdx;
        let gdy = rdh.div_euclid(2) + rdy;
        Ok(Rc::new(decode_refinement(mq, self.gr_cx, nw as usize, nh as usize, self.r_template, base, gdx, gdy, false, self.r_at)?))
    }
}

fn decode_text(src: &mut dyn TextSrc, w: usize, h: usize, p: &TextParams, syms: &[Rc<Bitmap>], budget: &mut usize) -> R<Bitmap> {
    let mut page = Bitmap::new(w, h, p.def_pixel)?;
    let strips = 1i64 << p.log_strips;
    let first = src.dt()?;
    let mut stript = -first * strips;
    let mut firsts = 0i64;
    let mut n = 0u32;
    while n < p.num_instances {
        let dt = src.dt()?;
        stript += dt * strips;
        let mut first_in_strip = true;
        let mut curs = 0i64;
        loop {
            if first_in_strip {
                firsts += src.first_s()?;
                curs = firsts;
                first_in_strip = false;
            } else {
                match src.next_s()? {
                    None => break,
                    Some(ids) => curs += ids + p.ds_offset,
                }
            }
            if n >= p.num_instances {
                // a strip must end with the out-of-band value; more instances than announced is damage
                return Err("text region: more instances than announced".into());
            }
            let curt = if strips == 1 { 0 } else { src.curt()? };
            let ti = stript + curt;
            let id = src.id()?;
            let ri = if p.refine { src.ri()? } else { 0 };
            let base = syms.get(id).ok_or_else(|| format!("text region: symbol {id} does not exist"))?.clone();
            let ib: Rc<Bitmap> = if ri == 0 { base } else { src.refined(&base, budget)? };
            let (wi, hi) = (ib.w as i64, ib.h as i64);
            // ref_corner: 0 bottom left, 1 top left, 2 bottom right, 3 top right
            let right = p.ref_corner >= 2;
            let bottom = p.ref_corner & 1 == 0;
            if !p.transposed && right {
                curs += wi - 1;
            } else if p.transposed && bottom {
                curs += hi - 1;
            }
            let si = curs;
            let (x, y) = if !p.transposed {
                (if right { si - wi + 1 } else { si }, if bottom { ti - hi + 1 } else { ti })
            } else {
                (if right { ti - wi + 1 } else { ti }, if bottom { si - hi + 1 } else { si })
            };
            page.combine(&ib, x, y, p.comb_op);
            if !p.transposed && !right {
                curs += wi - 1;
            } else if p.transposed && !bottom {
                curs += hi - 1;
            }
            n += 1;
        }
    }
    Ok(page)
}

fn spend(budget: &mut usize, n: usize) -> R<()> {
    if n > *budget {
        return Err("the symbols together are too large".into());
    }
    *budget -= n;
    Ok(())
}

// ------------------------------------------------------------------------------------------------ symbol dictionary (6.5)

struct SymbolDict {
    exported: Vec<Rc<Bitmap>>,
    /// Retained arithmetic contexts for a dictionary that says it uses them: the generic ones and the refinement ones.
    gb_cx: Vec<u8>,
    gr_cx: Vec<u8>,
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> R<u8> {
        let v = *self.d.get(self.p).ok_or("segment is cut short")?;
        self.p += 1;
        Ok(v)
    }
    fn i8(&mut self) -> R<i8> {
        Ok(self.u8()? as i8)
    }
    fn u16(&mut self) -> R<u16> {
        Ok((self.u8()? as u16) << 8 | self.u8()? as u16)
    }
    fn u32(&mut self) -> R<u32> {
        Ok((self.u16()? as u32) << 16 | self.u16()? as u32)
    }
    fn at_pairs(&mut self, n: usize) -> R<Vec<(i8, i8)>> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push((self.i8()?, self.i8()?));
        }
        Ok(v)
    }
    fn rest(&self) -> &'a [u8] {
        &self.d[self.p.min(self.d.len())..]
    }
}

fn decode_symbol_dict(data: &[u8], inputs: &[Rc<Bitmap>], used: Option<&SymbolDict>, custom: &[Rc<huff::Table>], budget: &mut usize) -> R<SymbolDict> {
    let mut r = Reader { d: data, p: 0 };
    let flags = r.u16()?;
    let huff = flags & 1 != 0;
    let refagg = flags & 2 != 0;
    let ctx_used = flags & 0x100 != 0;
    let ctx_retained = flags & 0x200 != 0;
    let template = ((flags >> 10) & 3) as u8;
    let r_template = ((flags >> 12) & 1) as u8;
    let at = if huff { Vec::new() } else { r.at_pairs(if template == 0 { 4 } else { 1 })? };
    let r_at = if refagg && r_template == 0 { r.at_pairs(2)? } else { vec![(-1, -1), (-1, -1)] };
    let num_ex = r.u32()? as usize;
    let num_new = r.u32()? as usize;
    if num_new > 1 << 20 || num_ex > 1 << 21 {
        return Err("symbol dictionary: too many symbols".into());
    }
    if huff {
        let gr_cx = match (ctx_used, used) {
            (true, Some(u)) if u.gr_cx.len() >= refinement_contexts(r_template) => u.gr_cx.clone(),
            _ => vec![0u8; refinement_contexts(r_template)],
        };
        let hh = huff::DictHeader { flags, refagg, r_template, r_at, num_new, gr_cx, custom };
        let (exported, gr_cx) = huff::symbol_dict(hh, r.rest(), inputs, budget)?;
        return Ok(SymbolDict { exported, gb_cx: Vec::new(), gr_cx: if ctx_retained { gr_cx } else { Vec::new() } });
    }
    let mut mq = Mq::new(r.rest());
    let (mut gb_cx, mut gr_cx) = match (ctx_used, used) {
        (true, Some(u)) => (u.gb_cx.clone(), u.gr_cx.clone()),
        _ => (vec![0u8; generic_contexts(template)], vec![0u8; refinement_contexts(r_template)]),
    };
    if gb_cx.len() < generic_contexts(template) {
        gb_cx.resize(generic_contexts(template), 0);
    }
    if gr_cx.len() < refinement_contexts(r_template) {
        gr_cx.resize(refinement_contexts(r_template), 0);
    }
    let total = inputs.len() + num_new;
    let sym_len = code_len(total);
    let mut dh = IntCx::new();
    let mut dw = IntCx::new();
    let mut ex = IntCx::new();
    let mut ai = IntCx::new();
    let mut tcx = TextCx::new(sym_len);
    let mut news: Vec<Rc<Bitmap>> = Vec::new();
    let mut hc_height = 0i64;
    while news.len() < num_new {
        hc_height += dh.decode(&mut mq).ok_or("symbol dictionary: out-of-band height")?;
        if !(0..=1 << 16).contains(&hc_height) {
            return Err("symbol dictionary: symbol height".into());
        }
        let mut sym_width = 0i64;
        loop {
            let Some(d) = dw.decode(&mut mq) else { break };
            if news.len() >= num_new {
                return Err("symbol dictionary: more symbols than announced".into());
            }
            sym_width += d;
            if !(0..=1 << 16).contains(&sym_width) {
                return Err("symbol dictionary: symbol width".into());
            }
            let (sw, sh) = (sym_width as usize, hc_height as usize);
            spend(budget, sw * sh)?;
            let bm = if !refagg {
                decode_generic(&mut mq, &mut gb_cx, sw, sh, template, false, &at)?
            } else {
                let ninst = ai.decode(&mut mq).ok_or("symbol dictionary: out-of-band instance count")?;
                let all = || inputs.iter().chain(news.iter()).cloned().collect::<Vec<_>>();
                if ninst > 1 {
                    let params = TextParams { refine: true, log_strips: 0, ref_corner: 1, transposed: false, comb_op: 0, def_pixel: 0, ds_offset: 0, r_at: r_at.clone(), num_instances: ninst as u32 };
                    let mut src = ArithText { mq: &mut mq, tcx: &mut tcx, gr_cx: &mut gr_cx, sym_code_len: sym_len, r_template, r_at: &r_at };
                    decode_text(&mut src, sw, sh, &params, &all(), budget)?
                } else {
                    let id = tcx.id.decode(&mut mq, sym_len);
                    let rdx = tcx.rdx.decode(&mut mq).ok_or("symbol dictionary: out-of-band dx")?;
                    let rdy = tcx.rdy.decode(&mut mq).ok_or("symbol dictionary: out-of-band dy")?;
                    let base = inputs.iter().chain(news.iter()).nth(id).ok_or("symbol dictionary: refined symbol does not exist")?.clone();
                    decode_refinement(&mut mq, &mut gr_cx, sw, sh, r_template, &base, rdx, rdy, false, &r_at)?
                }
            };
            news.push(Rc::new(bm));
        }
    }
    // which of the input and new symbols are exported: runs of "no" and "yes" in turn
    let mut flags_ex = vec![false; total];
    let (mut i, mut cur) = (0usize, false);
    while i < total {
        let run = ex.decode(&mut mq).ok_or("symbol dictionary: out-of-band export run")?;
        if run < 0 || i + run as usize > total {
            return Err("symbol dictionary: export run".into());
        }
        for f in &mut flags_ex[i..i + run as usize] {
            *f = cur;
        }
        i += run as usize;
        cur = !cur;
    }
    let exported: Vec<Rc<Bitmap>> = inputs.iter().chain(news.iter()).zip(&flags_ex).filter(|(_, f)| **f).map(|(b, _)| b.clone()).collect();
    // an encoder that counts differently from the header is not fatal: the flags are what the standard says is exported
    let _ = num_ex;
    Ok(SymbolDict { exported, gb_cx: if ctx_retained { gb_cx } else { Vec::new() }, gr_cx: if ctx_retained { gr_cx } else { Vec::new() } })
}

// ------------------------------------------------------------------------------------------------ segments and the page

struct SegHeader {
    number: u32,
    kind: u8,
    refs: Vec<u32>,
    data_len: Option<usize>,
}

fn parse_header(r: &mut Reader) -> R<SegHeader> {
    let number = r.u32()?;
    let flags = r.u8()?;
    let kind = flags & 0x3F;
    let big_page = flags & 0x40 != 0;
    let first = r.u8()?;
    let mut count = (first >> 5) as usize;
    if count == 7 {
        r.p -= 1;
        count = (r.u32()? & 0x1FFF_FFFF) as usize;
        r.p += (count + 1).div_ceil(8);
    }
    if count > 1 << 16 {
        return Err("segment refers to too many segments".into());
    }
    let size = if number <= 256 {
        1
    } else if number <= 65536 {
        2
    } else {
        4
    };
    let mut refs = Vec::with_capacity(count);
    for _ in 0..count {
        refs.push(match size {
            1 => r.u8()? as u32,
            2 => r.u16()? as u32,
            _ => r.u32()?,
        });
    }
    if big_page {
        r.u32()?;
    } else {
        r.u8()?;
    }
    let len = r.u32()?;
    Ok(SegHeader { number, kind, refs, data_len: if len == 0xFFFF_FFFF { None } else { Some(len as usize) } })
}

struct RegionInfo {
    w: usize,
    h: usize,
    x: i64,
    y: i64,
    op: u8,
}

fn region_info(r: &mut Reader) -> R<RegionInfo> {
    let w = r.u32()? as usize;
    let h = r.u32()? as usize;
    let x = r.u32()? as i32 as i64;
    let y = r.u32()? as i32 as i64;
    let flags = r.u8()?;
    if w.checked_mul(h).is_none_or(|n| n > MAX_PIXELS) {
        return Err("region is too large".into());
    }
    Ok(RegionInfo { w, h, x, y, op: flags & 7 })
}

/// For a generic region of unknown length: the end is the marker that closes the data and the four bytes of row count after it.
fn unknown_length(d: &[u8], mmr: bool) -> usize {
    let marker: [u8; 2] = if mmr { [0x00, 0x00] } else { [0xFF, 0xAC] };
    let mut i = 18;
    while i + 1 < d.len() {
        if d[i] == marker[0] && d[i + 1] == marker[1] {
            return (i + 6).min(d.len());
        }
        i += 1;
    }
    d.len()
}

/// An MMR-coded bitmap (the fax two-dimensional code, black is one) through the fax decoder.
fn mmr_bitmap_of(data: &[u8], w: usize, h: usize) -> R<Bitmap> {
    let p = ccitt::Params { k: -1, columns: w.max(1), rows: h, black_is_1: true, byte_align: false };
    let (bits, _) = ccitt::decode(data, &p, MAX_PIXELS);
    bits_to_bitmap(&bits, w, h)
}

/// Rows of one bit per pixel, padded to a byte, as the fax decoder gives them, as a bitmap.
fn bits_to_bitmap(bits: &[u8], w: usize, h: usize) -> R<Bitmap> {
    let row = w.div_ceil(8).max(1);
    let mut bm = Bitmap::new(w, h, 0)?;
    for y in 0..h {
        for x in 0..w {
            let b = bits.get(y * row + x / 8).copied().unwrap_or(0);
            bm.d[y * w + x] = (b >> (7 - x % 8)) & 1;
        }
    }
    Ok(bm)
}

#[derive(Default)]
struct Decoder {
    page: Option<Bitmap>,
    page_def: u8,
    unknown_height: bool,
    dicts: HashMap<u32, Rc<SymbolDict>>,
    patterns: HashMap<u32, Rc<Vec<Bitmap>>>,
    regions: HashMap<u32, Bitmap>,
    /// Custom code tables (segment type 53) by segment number.
    tables: HashMap<u32, Rc<huff::Table>>,
    budget: usize,
    /// The first thing that could not be read, kept so the page still gives what was read.
    problem: Option<String>,
}

impl Decoder {
    fn run(&mut self, d: &[u8]) {
        let mut pos = 0usize;
        // an optional file header (the stand-alone file form): 8 id bytes, flags, and the page count unless it is unknown
        if d.starts_with(&[0x97, 0x4A, 0x42, 0x32, 0x0D, 0x0A, 0x1A, 0x0A]) {
            pos = 9 + if d.get(8).is_some_and(|f| f & 2 == 0) { 4 } else { 0 };
        }
        while pos < d.len() {
            let mut r = Reader { d, p: pos };
            let Ok(h) = parse_header(&mut r) else { break };
            let start = r.p;
            let len = match h.data_len {
                Some(l) => l,
                None => {
                    let mmr = d.get(start + 17).is_some_and(|f| f & 1 != 0);
                    unknown_length(&d[start.min(d.len())..], mmr)
                }
            };
            let end = start.saturating_add(len).min(d.len());
            let body = &d[start.min(d.len())..end];
            pos = end;
            if h.kind == 51 {
                break;
            }
            if let Err(e) = self.segment(&h, body) {
                self.problem.get_or_insert(e);
            }
        }
    }

    fn segment(&mut self, h: &SegHeader, body: &[u8]) -> R<()> {
        match h.kind {
            0 => {
                let mut inputs: Vec<Rc<Bitmap>> = Vec::new();
                let mut used: Option<Rc<SymbolDict>> = None;
                for n in &h.refs {
                    if let Some(sd) = self.dicts.get(n) {
                        inputs.extend(sd.exported.iter().cloned());
                        used = Some(sd.clone());
                    }
                }
                let custom = self.custom_tables(h);
                let sd = decode_symbol_dict(body, &inputs, used.as_deref(), &custom, &mut self.budget)?;
                self.dicts.insert(h.number, Rc::new(sd));
            }
            4 | 6 | 7 => self.text_region(h, body)?,
            16 => self.pattern_dict(h, body)?,
            20 | 22 | 23 => self.halftone_region(h, body)?,
            36 | 38 | 39 => self.generic_region(h, body)?,
            40 | 42 | 43 => self.refinement_region(h, body)?,
            48 => {
                let mut r = Reader { d: body, p: 0 };
                let w = r.u32()? as usize;
                let hgt = r.u32()?;
                r.u32()?;
                r.u32()?;
                let flags = r.u8()?;
                self.page_def = (flags >> 2) & 1;
                self.unknown_height = hgt == 0xFFFF_FFFF;
                self.page = Some(Bitmap::new(w, if self.unknown_height { 0 } else { hgt as usize }, self.page_def)?);
            }
            50 => {
                let mut r = Reader { d: body, p: 0 };
                let y = r.u32()? as usize + 1;
                if self.unknown_height {
                    let def = self.page_def;
                    if let Some(p) = self.page.as_mut() {
                        p.grow(y, def)?;
                    }
                }
            }
            53 => {
                let t = huff::custom_table(body)?;
                self.tables.insert(h.number, Rc::new(t));
            }
            _ => {}
        }
        Ok(())
    }

    /// The custom code tables a segment refers to, in the order it refers to them.
    fn custom_tables(&self, h: &SegHeader) -> Vec<Rc<huff::Table>> {
        h.refs.iter().filter_map(|n| self.tables.get(n).cloned()).collect()
    }

    fn place(&mut self, h: &SegHeader, info: &RegionInfo, bm: Bitmap) -> R<()> {
        // intermediate regions (types 4, 20, 36, 40) are kept for a later segment; immediate ones go on the page
        if matches!(h.kind, 4 | 20 | 36 | 40) {
            self.regions.insert(h.number, bm);
            return Ok(());
        }
        let def = self.page_def;
        let unknown = self.unknown_height;
        let page = self.page.as_mut().ok_or("a region came before the page information")?;
        if unknown {
            page.grow((info.y.max(0) as usize).saturating_add(info.h), def)?;
        }
        page.combine(&bm, info.x, info.y, info.op);
        Ok(())
    }

    fn generic_region(&mut self, h: &SegHeader, body: &[u8]) -> R<()> {
        let mut r = Reader { d: body, p: 0 };
        let info = region_info(&mut r)?;
        let flags = r.u8()?;
        let mmr = flags & 1 != 0;
        let template = (flags >> 1) & 3;
        let tpgdon = flags & 8 != 0;
        let bm = if mmr {
            self.mmr_bitmap(r.rest(), info.w, info.h)?
        } else {
            let at = r.at_pairs(if template == 0 { 4 } else { 1 })?;
            let mut mq = Mq::new(r.rest());
            let mut cx = vec![0u8; generic_contexts(template)];
            decode_generic(&mut mq, &mut cx, info.w, info.h, template, tpgdon, &at)?
        };
        self.place(h, &info, bm)
    }

    fn pattern_dict(&mut self, h: &SegHeader, body: &[u8]) -> R<()> {
        let mut r = Reader { d: body, p: 0 };
        let flags = r.u8()?;
        let (mmr, template) = (flags & 1 != 0, (flags >> 1) & 3);
        let (pw, ph) = (r.u8()? as usize, r.u8()? as usize);
        let gray_max = r.u32()? as usize;
        if pw == 0 || ph == 0 || gray_max >= 1 << 16 {
            return Err("pattern dictionary: size".into());
        }
        let w = (gray_max + 1) * pw;
        let all = if mmr {
            self.mmr_bitmap(r.rest(), w, ph)?
        } else {
            let at = [((-(pw.min(128) as i16)) as i8, 0), (-3, -1), (2, -2), (-2, -2)];
            let mut mq = Mq::new(r.rest());
            let mut cx = vec![0u8; generic_contexts(template)];
            decode_generic(&mut mq, &mut cx, w, ph, template, false, &at)?
        };
        let pats = (0..=gray_max).map(|k| all.sub((k * pw) as i64, 0, pw, ph)).collect::<R<Vec<_>>>()?;
        self.patterns.insert(h.number, Rc::new(pats));
        Ok(())
    }

    fn halftone_region(&mut self, h: &SegHeader, body: &[u8]) -> R<()> {
        let mut r = Reader { d: body, p: 0 };
        let info = region_info(&mut r)?;
        let flags = r.u8()?;
        let (mmr, template, skip, op, def) = (flags & 1 != 0, (flags >> 1) & 3, flags & 8 != 0, (flags >> 4) & 7, (flags >> 7) & 1);
        let (gw, gh) = (r.u32()? as usize, r.u32()? as usize);
        let (gx, gy) = (r.u32()? as i32 as i64, r.u32()? as i32 as i64);
        let (rx, ry) = (r.u16()? as i64, r.u16()? as i64);
        let pats = h.refs.iter().find_map(|n| self.patterns.get(n)).cloned().ok_or("halftone region: no pattern dictionary")?;
        if gw.checked_mul(gh).is_none_or(|n| n > MAX_PIXELS) {
            return Err("halftone region: grid is too large".into());
        }
        let bpp = code_len(pats.len()).max(1) as usize;
        let mut region = Bitmap::new(info.w, info.h, def)?;
        let (hpw, hph) = (pats[0].w as i64, pats[0].h as i64);
        // the grid cell (m, n) sits at this pixel; a pattern that falls wholly outside the region is not coded when the skip bit is set (6.6.5.1)
        let place_of = |m: i64, n: i64| ((gx + m * ry + n * rx) >> 8, (gy + m * rx - n * ry) >> 8);
        let skip_map = if skip {
            let mut sk = Bitmap::new(gw, gh, 0)?;
            for m in 0..gh as i64 {
                for n in 0..gw as i64 {
                    let (x, y) = place_of(m, n);
                    if x + hpw <= 0 || x >= info.w as i64 || y + hph <= 0 || y >= info.h as i64 {
                        sk.d[m as usize * gw + n as usize] = 1;
                    }
                }
            }
            Some(sk)
        } else {
            None
        };
        let at: Vec<(i8, i8)> = if template <= 1 { vec![(3, -1), (-3, -1), (2, -2), (-2, -2)] } else { vec![(2, -1), (-3, -1), (2, -2), (-2, -2)] };
        // the gray-scale image (Annex C): bit planes from the most significant down, each Gray coded against the one before it
        let mut planes: Vec<Bitmap> = Vec::new();
        if mmr {
            // the planes lie one after the other, each ending at a byte boundary after its end-of-block code
            let mut rest = r.rest();
            for _ in 0..bpp {
                let p = ccitt::Params { k: -1, columns: gw.max(1), rows: gh, black_is_1: true, byte_align: false };
                let (bits, _, used) = ccitt::decode_counting(rest, &p, MAX_PIXELS);
                planes.push(bits_to_bitmap(&bits, gw, gh)?);
                rest = &rest[used.min(rest.len())..];
            }
        } else {
            let mut mq = Mq::new(r.rest());
            let mut cx = vec![0u8; generic_contexts(template)];
            for _ in 0..bpp {
                planes.push(decode_generic_skip(&mut mq, &mut cx, gw, gh, template, false, &at, skip_map.as_ref())?);
            }
        }
        for k in 1..bpp {
            let upper = planes[k - 1].d.clone();
            for (a, b) in planes[k].d.iter_mut().zip(upper) {
                *a ^= b;
            }
        }
        for m in 0..gh as i64 {
            for n in 0..gw as i64 {
                let mut v = 0usize;
                for (k, pl) in planes.iter().enumerate() {
                    v |= (pl.d[m as usize * gw + n as usize] as usize) << (bpp - 1 - k);
                }
                let (x, y) = place_of(m, n);
                region.combine(&pats[v.min(pats.len() - 1)], x, y, op);
            }
        }
        self.place(h, &info, region)
    }

    fn mmr_bitmap(&self, data: &[u8], w: usize, h: usize) -> R<Bitmap> {
        mmr_bitmap_of(data, w, h)
    }

    fn refinement_region(&mut self, h: &SegHeader, body: &[u8]) -> R<()> {
        let mut r = Reader { d: body, p: 0 };
        let info = region_info(&mut r)?;
        let flags = r.u8()?;
        let template = flags & 1;
        let tpgron = flags & 2 != 0;
        let at = if template == 0 { r.at_pairs(2)? } else { vec![(-1, -1), (-1, -1)] };
        let reference = match h.refs.iter().find_map(|n| self.regions.get(n)) {
            Some(b) => b.clone(),
            None => {
                let page = self.page.as_ref().ok_or("a refinement region came before the page information")?;
                page.sub(info.x, info.y, info.w, info.h)?
            }
        };
        let mut mq = Mq::new(r.rest());
        let mut cx = vec![0u8; refinement_contexts(template)];
        let bm = decode_refinement(&mut mq, &mut cx, info.w, info.h, template, &reference, 0, 0, tpgron, &at)?;
        self.place(h, &info, bm)
    }

    fn text_region(&mut self, h: &SegHeader, body: &[u8]) -> R<()> {
        let mut r = Reader { d: body, p: 0 };
        let info = region_info(&mut r)?;
        let flags = r.u16()?;
        let huff = flags & 1 != 0;
        let hflags = if huff { r.u16()? } else { 0 };
        let refine = flags & 2 != 0;
        let r_template = ((flags >> 15) & 1) as u8;
        let mut ds = ((flags >> 10) & 0x1F) as i64;
        if ds >= 16 {
            ds -= 32;
        }
        let r_at = if refine && r_template == 0 { r.at_pairs(2)? } else { vec![(-1, -1), (-1, -1)] };
        let num_instances = r.u32()?;
        let p = TextParams {
            refine,
            log_strips: ((flags >> 2) & 3) as u32,
            ref_corner: ((flags >> 4) & 3) as u8,
            transposed: flags & 0x40 != 0,
            comb_op: ((flags >> 7) & 3) as u8,
            def_pixel: ((flags >> 9) & 1) as u8,
            ds_offset: ds,
            r_at,
            num_instances,
        };
        let mut syms: Vec<Rc<Bitmap>> = Vec::new();
        for n in &h.refs {
            if let Some(sd) = self.dicts.get(n) {
                syms.extend(sd.exported.iter().cloned());
            }
        }
        let len = code_len(syms.len());
        let mut gr_cx = vec![0u8; refinement_contexts(r_template)];
        let mut budget = self.budget;
        let bm = if huff {
            let custom = self.custom_tables(h);
            let tables = huff::text_tables(hflags, &mut huff::Picker::new(&custom))?;
            let mut bits = huff::Bits::new(r.rest());
            let codes = huff::SymCodes::Table(huff::read_symbol_codes(&mut bits, syms.len())?);
            let mut src = huff::HuffText { bits: &mut bits, t: &tables, codes: &codes, log_strips: p.log_strips, refine, gr_cx: &mut gr_cx, r_template, r_at: &p.r_at };
            decode_text(&mut src, info.w, info.h, &p, &syms, &mut budget)?
        } else {
            let mut mq = Mq::new(r.rest());
            let mut tcx = TextCx::new(len);
            let mut src = ArithText { mq: &mut mq, tcx: &mut tcx, gr_cx: &mut gr_cx, sym_code_len: len, r_template, r_at: &p.r_at };
            decode_text(&mut src, info.w, info.h, &p, &syms, &mut budget)?
        };
        self.place(h, &info, bm)
    }
}

/// Decode an embedded JBIG2 stream (the form the PDF filter has: segments one after the other, no file header) with the global
/// stream the filter's parameters name, if any. The page is returned even when a later segment was damaged or is of a kind that is
/// not read; the second value then says what went wrong.
pub fn decode(data: &[u8], globals: Option<&[u8]>) -> R<(Bitmap, Option<String>)> {
    let mut dec = Decoder { budget: MAX_SYMBOL_PIXELS, ..Default::default() };
    if let Some(g) = globals {
        dec.run(g);
    }
    dec.run(data);
    let problem = dec.problem.take();
    match dec.page {
        Some(p) => Ok((p, problem)),
        None => Err(problem.unwrap_or_else(|| "no page information in the JBIG2 data".into())),
    }
}

/// Pack a page as the PDF filter gives it: rows of one bit per pixel padded to a byte, 0 for black and 1 for white.
pub fn pack_pdf(page: &Bitmap, width: usize, height: usize) -> Vec<u8> {
    let row = width.div_ceil(8);
    let mut out = vec![0xFFu8; row * height];
    for y in 0..height.min(page.h) {
        for x in 0..width.min(page.w) {
            if page.d[y * page.w + x] != 0 {
                out[y * row + x / 8] &= !(0x80 >> (x % 8));
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "jbig2_tests.rs"]
pub(crate) mod jbig2_tests;
