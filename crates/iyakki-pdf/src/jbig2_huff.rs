//! The Huffman side of JBIG2 (ITU-T T.88, F-PDF-17): bit reading, prefix tables built by the code assignment of B.3, the fifteen
//! standard tables of Annex B.5, custom code tables (segment type 53), Huffman-coded symbol dictionaries (6.5, with the collective
//! bitmap of 6.5.9) and Huffman-coded text regions (6.4, symbol code lengths of 7.4.3.1.7).
//!
//! Where the standard tables come from: the line values (range start, prefix length, range length, lower and upper range lines and
//! the out-of-band line) were taken by a script from the table definitions of an openly licensed decoder (Apache-2.0, pdf.js,
//! `src/core/jbig2.js`, `getStandardTable`), not typed in. They are checked three ways by the tests: every table's prefix lengths
//! fill the code space exactly, the codes the B.3 assignment gives equal the explicit codes that source lists, and streams coded
//! with them are read by Ghostscript to the same picture. The text of T.88 itself could not be fetched here (the ITU page needs a
//! login), see the row's notes.

use std::rc::Rc;

use super::{decode_refinement, spend, Bitmap, Mq, TextParams, TextSrc, R};

/// MSB-first bit reader over a segment's data.
pub(super) struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub(super) fn new(d: &'a [u8]) -> Bits<'a> {
        Bits { d, pos: 0 }
    }
    pub(super) fn bit(&mut self) -> R<u32> {
        let b = *self.d.get(self.pos / 8).ok_or("Huffman data is cut short")?;
        let v = (b >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Ok(v as u32)
    }
    pub(super) fn bits(&mut self, n: u32) -> R<u32> {
        let mut v = 0u64;
        for _ in 0..n {
            v = v << 1 | self.bit()? as u64;
        }
        Ok(v as u32)
    }
    pub(super) fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }
    /// The bytes from the next byte boundary, `n` of them, and the reader moved past them.
    pub(super) fn take_bytes(&mut self, n: usize) -> R<&'a [u8]> {
        self.align();
        let s = self.pos / 8;
        let e = s.checked_add(n).filter(|e| *e <= self.d.len()).ok_or("Huffman data is cut short")?;
        self.pos = e * 8;
        Ok(&self.d[s..e])
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Normal,
    Lower,
    Upper,
    Oob,
}

#[derive(Clone, Debug)]
struct Line {
    low: i64,
    preflen: u8,
    rangelen: u8,
    kind: Kind,
    code: u32,
}

/// A prefix table: lines in the standard's order, codes assigned by B.3.
#[derive(Debug)]
pub(super) struct Table {
    lines: Vec<Line>,
}

impl Table {
    /// Assign the codes of B.3 to the lines in their order (a line with prefix length 0 is not used).
    fn assign(mut lines: Vec<Line>) -> R<Table> {
        let max = lines.iter().map(|l| l.preflen).max().unwrap_or(0) as usize;
        if max > 32 {
            return Err("Huffman table: prefix longer than 32 bits".into());
        }
        let mut count = vec![0u64; max + 2];
        for l in &lines {
            if l.preflen > 0 {
                count[l.preflen as usize] += 1;
            }
        }
        let mut first = 0u64;
        for len in 1..=max {
            first = (first + count[len - 1]) << 1;
            let mut cur = first;
            for l in lines.iter_mut().filter(|l| l.preflen as usize == len) {
                if cur >= 1 << len {
                    return Err("Huffman table: the prefix lengths overfill the code space".into());
                }
                l.code = cur as u32;
                cur += 1;
            }
            // count[len] is the number of lines of this length: the next first code starts after them
        }
        Ok(Table { lines })
    }

    /// A table whose line `i` is the value `i` with the given prefix length (symbol codes of a text region, run codes).
    pub(super) fn of_lengths(lens: &[u8]) -> R<Table> {
        Table::assign(lens.iter().enumerate().map(|(i, &l)| Line { low: i as i64, preflen: l, rangelen: 0, kind: Kind::Normal, code: 0 }).collect())
    }

    /// Read one value: `None` is the out-of-band value.
    pub(super) fn decode(&self, b: &mut Bits) -> R<Option<i64>> {
        let mut code = 0u32;
        let mut len = 0u8;
        loop {
            code = code << 1 | b.bit()?;
            len += 1;
            if len > 32 {
                return Err("Huffman code not found in its table".into());
            }
            for l in &self.lines {
                if l.preflen == len && l.code == code {
                    return Ok(match l.kind {
                        Kind::Oob => None,
                        Kind::Normal => Some(l.low + b.bits(l.rangelen as u32)? as i64),
                        Kind::Lower => Some(l.low - b.bits(32)? as i64),
                        Kind::Upper => Some(l.low + b.bits(32)? as i64),
                    });
                }
            }
        }
    }

    /// The sum of 2^-length over the lines: 1 when the prefix lengths fill the code space exactly.
    #[cfg(test)]
    pub(super) fn kraft(&self) -> f64 {
        self.lines.iter().filter(|l| l.preflen > 0).map(|l| 0.5f64.powi(l.preflen as i32)).sum()
    }

    /// The codes of the lines in table order, for the tests.
    #[cfg(test)]
    pub(super) fn line_codes(&self) -> Vec<u32> {
        self.lines.iter().map(|l| l.code).collect()
    }

    /// Append the bits that code `v` (`None` is out of band), for the tests' encoders; false when the table has no line for the value.
    #[cfg(test)]
    pub(super) fn encode(&self, out: &mut Vec<bool>, v: Option<i64>) -> bool {
        let put = |out: &mut Vec<bool>, x: u64, n: u32| out.extend((0..n).rev().map(|i| x >> i & 1 == 1));
        let line = match v {
            None => self.lines.iter().find(|l| l.kind == Kind::Oob && l.preflen > 0),
            Some(v) => self
                .lines
                .iter()
                .find(|l| l.kind == Kind::Normal && l.preflen > 0 && v >= l.low && v < l.low + (1i64 << l.rangelen))
                .or_else(|| self.lines.iter().find(|l| l.kind == Kind::Lower && l.preflen > 0 && v <= l.low))
                .or_else(|| self.lines.iter().find(|l| l.kind == Kind::Upper && l.preflen > 0 && v >= l.low)),
        };
        let Some(l) = line else { return false };
        put(out, l.code as u64, l.preflen as u32);
        match (l.kind, v) {
            (Kind::Normal, Some(v)) => put(out, (v - l.low) as u64, l.rangelen as u32),
            (Kind::Lower, Some(v)) => put(out, (l.low - v) as u64, 32),
            (Kind::Upper, Some(v)) => put(out, (v - l.low) as u64, 32),
            _ => {}
        }
        true
    }

    /// Read a value that must not be out of band.
    fn value(&self, b: &mut Bits, what: &str) -> R<i64> {
        self.decode(b)?.ok_or_else(|| format!("Huffman data: out-of-band {what}"))
    }
}

struct Spec {
    normal: &'static [(i64, u8, u8)],
    lower: Option<(i64, u8)>,
    upper: Option<(i64, u8)>,
    oob: Option<u8>,
}

/// Annex B.5, tables B.1 to B.15 in order.
const STANDARD: [Spec; 15] = [
    Spec { normal: &[(0, 1, 4), (16, 2, 8), (272, 3, 16)], lower: None, upper: Some((65808, 3)), oob: None }, // B.1
    Spec { normal: &[(0, 1, 0), (1, 2, 0), (2, 3, 0), (3, 4, 3), (11, 5, 6)], lower: None, upper: Some((75, 6)), oob: Some(6) }, // B.2
    Spec { normal: &[(-256, 8, 8), (0, 1, 0), (1, 2, 0), (2, 3, 0), (3, 4, 3), (11, 5, 6)], lower: Some((-257, 8)), upper: Some((75, 7)), oob: Some(6) }, // B.3
    Spec { normal: &[(1, 1, 0), (2, 2, 0), (3, 3, 0), (4, 4, 3), (12, 5, 6)], lower: None, upper: Some((76, 5)), oob: None }, // B.4
    Spec { normal: &[(-255, 7, 8), (1, 1, 0), (2, 2, 0), (3, 3, 0), (4, 4, 3), (12, 5, 6)], lower: Some((-256, 7)), upper: Some((76, 6)), oob: None }, // B.5
    Spec { normal: &[(-2048, 5, 10), (-1024, 4, 9), (-512, 4, 8), (-256, 4, 7), (-128, 5, 6), (-64, 5, 5), (-32, 4, 5), (0, 2, 7), (128, 3, 7), (256, 3, 8), (512, 4, 9), (1024, 4, 10)], lower: Some((-2049, 6)), upper: Some((2048, 6)), oob: None }, // B.6
    Spec { normal: &[(-1024, 4, 9), (-512, 3, 8), (-256, 4, 7), (-128, 5, 6), (-64, 5, 5), (-32, 4, 5), (0, 4, 5), (32, 5, 5), (64, 5, 6), (128, 4, 7), (256, 3, 8), (512, 3, 9), (1024, 3, 10)], lower: Some((-1025, 5)), upper: Some((2048, 5)), oob: None }, // B.7
    Spec { normal: &[(-15, 8, 3), (-7, 9, 1), (-5, 8, 1), (-3, 9, 0), (-2, 7, 0), (-1, 4, 0), (0, 2, 1), (2, 5, 0), (3, 6, 0), (4, 3, 4), (20, 6, 1), (22, 4, 4), (38, 4, 5), (70, 5, 6), (134, 5, 7), (262, 6, 7), (390, 7, 8), (646, 6, 10)], lower: Some((-16, 9)), upper: Some((1670, 9)), oob: Some(2) }, // B.8
    Spec { normal: &[(-31, 8, 4), (-15, 9, 2), (-11, 8, 2), (-7, 9, 1), (-5, 7, 1), (-3, 4, 1), (-1, 3, 1), (1, 3, 1), (3, 5, 1), (5, 6, 1), (7, 3, 5), (39, 6, 2), (43, 4, 5), (75, 4, 6), (139, 5, 7), (267, 5, 8), (523, 6, 8), (779, 7, 9), (1291, 6, 11)], lower: Some((-32, 9)), upper: Some((3339, 9)), oob: Some(2) }, // B.9
    Spec { normal: &[(-21, 7, 4), (-5, 8, 0), (-4, 7, 0), (-3, 5, 0), (-2, 2, 2), (2, 5, 0), (3, 6, 0), (4, 7, 0), (5, 8, 0), (6, 2, 6), (70, 5, 5), (102, 6, 5), (134, 6, 6), (198, 6, 7), (326, 6, 8), (582, 6, 9), (1094, 6, 10), (2118, 7, 11)], lower: Some((-22, 8)), upper: Some((4166, 8)), oob: Some(2) }, // B.10
    Spec { normal: &[(1, 1, 0), (2, 2, 1), (4, 4, 0), (5, 4, 1), (7, 5, 1), (9, 5, 2), (13, 6, 2), (17, 7, 2), (21, 7, 3), (29, 7, 4), (45, 7, 5), (77, 7, 6)], lower: None, upper: Some((141, 7)), oob: None }, // B.11
    Spec { normal: &[(1, 1, 0), (2, 2, 0), (3, 3, 1), (5, 5, 0), (6, 5, 1), (8, 6, 1), (10, 7, 0), (11, 7, 1), (13, 7, 2), (17, 7, 3), (25, 7, 4), (41, 8, 5)], lower: None, upper: Some((73, 8)), oob: None }, // B.12
    Spec { normal: &[(1, 1, 0), (2, 3, 0), (3, 4, 0), (4, 5, 0), (5, 4, 1), (7, 3, 3), (15, 6, 1), (17, 6, 2), (21, 6, 3), (29, 6, 4), (45, 6, 5), (77, 7, 6)], lower: None, upper: Some((141, 7)), oob: None }, // B.13
    Spec { normal: &[(-2, 3, 0), (-1, 3, 0), (0, 1, 0), (1, 3, 0), (2, 3, 0)], lower: None, upper: None, oob: None }, // B.14
    Spec { normal: &[(-24, 7, 4), (-8, 6, 2), (-4, 5, 1), (-2, 4, 0), (-1, 3, 0), (0, 1, 0), (1, 3, 0), (2, 4, 0), (3, 5, 1), (5, 6, 2), (9, 7, 4)], lower: Some((-25, 7)), upper: Some((25, 7)), oob: None }, // B.15
];

/// The codes the standard tables carry in the source they were taken from, in line order (normal lines, lower, upper, out of band).
#[cfg(test)]
pub(super) const SOURCE_CODES: [&[u32]; 15] = [
    &[0x0, 0x2, 0x6, 0x7], // B.1
    &[0x0, 0x2, 0x6, 0xe, 0x1e, 0x3e, 0x3f], // B.2
    &[0xfe, 0x0, 0x2, 0x6, 0xe, 0x1e, 0xff, 0x7e, 0x3e], // B.3
    &[0x0, 0x2, 0x6, 0xe, 0x1e, 0x1f], // B.4
    &[0x7e, 0x0, 0x2, 0x6, 0xe, 0x1e, 0x7f, 0x3e], // B.5
    &[0x1c, 0x8, 0x9, 0xa, 0x1d, 0x1e, 0xb, 0x0, 0x2, 0x3, 0xc, 0xd, 0x3e, 0x3f], // B.6
    &[0x8, 0x0, 0x9, 0x1a, 0x1b, 0xa, 0xb, 0x1c, 0x1d, 0xc, 0x1, 0x2, 0x3, 0x1e, 0x1f], // B.7
    &[0xfc, 0x1fc, 0xfd, 0x1fd, 0x7c, 0xa, 0x0, 0x1a, 0x3a, 0x4, 0x3b, 0xb, 0xc, 0x1b, 0x1c, 0x3c, 0x7d, 0x3d, 0x1fe, 0x1ff, 0x1], // B.8
    &[0xfc, 0x1fc, 0xfd, 0x1fd, 0x7c, 0xa, 0x2, 0x3, 0x1a, 0x3a, 0x4, 0x3b, 0xb, 0xc, 0x1b, 0x1c, 0x3c, 0x7d, 0x3d, 0x1fe, 0x1ff, 0x0], // B.9
    &[0x7a, 0xfc, 0x7b, 0x18, 0x0, 0x19, 0x36, 0x7c, 0xfd, 0x1, 0x1a, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x7d, 0xfe, 0xff, 0x2], // B.10
    &[0x0, 0x2, 0xc, 0xd, 0x1c, 0x1d, 0x3c, 0x7a, 0x7b, 0x7c, 0x7d, 0x7e, 0x7f], // B.11
    &[0x0, 0x2, 0x6, 0x1c, 0x1d, 0x3c, 0x7a, 0x7b, 0x7c, 0x7d, 0x7e, 0xfe, 0xff], // B.12
    &[0x0, 0x4, 0xc, 0x1c, 0xd, 0x5, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x7e, 0x7f], // B.13
    &[0x4, 0x5, 0x0, 0x6, 0x7], // B.14
    &[0x7c, 0x3c, 0x1c, 0xc, 0x4, 0x0, 0x5, 0xd, 0x1d, 0x3d, 0x7d, 0x7e, 0x7f], // B.15
];

/// Standard table B.`n` (1 to 15).
pub(super) fn standard(n: usize) -> R<Rc<Table>> {
    let s = STANDARD.get(n.wrapping_sub(1)).ok_or("standard Huffman table does not exist")?;
    let mut lines: Vec<Line> = s.normal.iter().map(|&(low, preflen, rangelen)| Line { low, preflen, rangelen, kind: Kind::Normal, code: 0 }).collect();
    if let Some((low, preflen)) = s.lower {
        lines.push(Line { low, preflen, rangelen: 32, kind: Kind::Lower, code: 0 });
    }
    if let Some((low, preflen)) = s.upper {
        lines.push(Line { low, preflen, rangelen: 32, kind: Kind::Upper, code: 0 });
    }
    if let Some(preflen) = s.oob {
        lines.push(Line { low: 0, preflen, rangelen: 0, kind: Kind::Oob, code: 0 });
    }
    Ok(Rc::new(Table::assign(lines)?))
}

/// A custom code table segment (type 53, B.2).
pub(super) fn custom_table(body: &[u8]) -> R<Table> {
    if body.len() < 9 {
        return Err("code table segment is cut short".into());
    }
    let flags = body[0];
    let oob = flags & 1 != 0;
    let ps = (flags as u32 >> 1 & 7) + 1;
    let rs = (flags as u32 >> 4 & 7) + 1;
    let low = i32::from_be_bytes([body[1], body[2], body[3], body[4]]) as i64;
    let high = i32::from_be_bytes([body[5], body[6], body[7], body[8]]) as i64;
    if low > high {
        return Err("code table: low above high".into());
    }
    let mut b = Bits::new(&body[9..]);
    let mut lines = Vec::new();
    let mut cur = low;
    while cur < high {
        if lines.len() >= 1 << 16 {
            return Err("code table: too many lines".into());
        }
        let preflen = b.bits(ps)? as u8;
        let rangelen = b.bits(rs)?;
        if rangelen > 32 {
            return Err("code table: range length above 32".into());
        }
        lines.push(Line { low: cur, preflen, rangelen: rangelen as u8, kind: Kind::Normal, code: 0 });
        cur += 1i64 << rangelen;
    }
    let lower = b.bits(ps)? as u8;
    lines.push(Line { low: low - 1, preflen: lower, rangelen: 32, kind: Kind::Lower, code: 0 });
    let upper = b.bits(ps)? as u8;
    lines.push(Line { low: high, preflen: upper, rangelen: 32, kind: Kind::Upper, code: 0 });
    if oob {
        let p = b.bits(ps)? as u8;
        lines.push(Line { low: 0, preflen: p, rangelen: 0, kind: Kind::Oob, code: 0 });
    }
    Table::assign(lines)
}

/// Picks the tables a header names: the standard one by its selector or the next custom table of the segment's references.
pub(super) struct Picker<'a> {
    pub custom: &'a [Rc<Table>],
    next: usize,
}

impl<'a> Picker<'a> {
    pub(super) fn new(custom: &'a [Rc<Table>]) -> Picker<'a> {
        Picker { custom, next: 0 }
    }
    /// `standards` lists the standard table numbers for the selector values 0, 1, 2; selector 3 takes the next custom table.
    pub(super) fn pick(&mut self, selector: u32, standards: &[usize]) -> R<Rc<Table>> {
        if selector == 3 {
            let t = self.custom.get(self.next).ok_or("a user supplied Huffman table is named but no table segment is referred to")?.clone();
            self.next += 1;
            return Ok(t);
        }
        match standards.get(selector as usize) {
            Some(&n) => standard(n),
            None => Err("Huffman table selector is not defined".into()),
        }
    }
}

// ------------------------------------------------------------------------------------------------ text region, Huffman

pub(super) struct TextTables {
    pub fs: Rc<Table>,
    pub ds: Rc<Table>,
    pub dt: Rc<Table>,
    pub rdw: Rc<Table>,
    pub rdh: Rc<Table>,
    pub rdx: Rc<Table>,
    pub rdy: Rc<Table>,
    pub rsize: Rc<Table>,
}

impl TextTables {
    /// The tables a refinement-and-aggregation symbol of a symbol dictionary uses (6.5.8.2.2).
    pub(super) fn for_aggregates() -> R<TextTables> {
        Ok(TextTables { fs: standard(6)?, ds: standard(8)?, dt: standard(11)?, rdw: standard(15)?, rdh: standard(15)?, rdx: standard(15)?, rdy: standard(15)?, rsize: standard(1)? })
    }
}

/// How the symbol of an instance is named in Huffman data.
pub(super) enum SymCodes {
    /// `n` bits, the symbol number itself (symbol dictionaries).
    Fixed(u32),
    /// A prefix table over the symbol numbers (text regions).
    Table(Table),
}

pub(super) struct HuffText<'b, 'a> {
    pub bits: &'b mut Bits<'a>,
    pub t: &'b TextTables,
    pub codes: &'b SymCodes,
    pub log_strips: u32,
    pub refine: bool,
    pub gr_cx: &'b mut [u8],
    pub r_template: u8,
    pub r_at: &'b [(i8, i8)],
}

impl TextSrc for HuffText<'_, '_> {
    fn dt(&mut self) -> R<i64> {
        self.t.dt.value(self.bits, "strip")
    }
    fn first_s(&mut self) -> R<i64> {
        self.t.fs.value(self.bits, "first S")
    }
    fn next_s(&mut self) -> R<Option<i64>> {
        self.t.ds.decode(self.bits)
    }
    fn curt(&mut self) -> R<i64> {
        Ok(self.bits.bits(self.log_strips)? as i64)
    }
    fn id(&mut self) -> R<usize> {
        match self.codes {
            SymCodes::Fixed(n) => Ok(self.bits.bits(*n)? as usize),
            SymCodes::Table(t) => Ok(t.value(self.bits, "symbol")? as usize),
        }
    }
    fn ri(&mut self) -> R<u8> {
        if self.refine {
            Ok(self.bits.bit()? as u8)
        } else {
            Ok(0)
        }
    }
    fn refined(&mut self, base: &Rc<Bitmap>, budget: &mut usize) -> R<Rc<Bitmap>> {
        let rdw = self.t.rdw.value(self.bits, "width")?;
        let rdh = self.t.rdh.value(self.bits, "height")?;
        let rdx = self.t.rdx.value(self.bits, "dx")?;
        let rdy = self.t.rdy.value(self.bits, "dy")?;
        let size = self.t.rsize.value(self.bits, "size")?;
        if size < 0 {
            return Err("refinement size below zero".into());
        }
        let nw = base.w as i64 + rdw;
        let nh = base.h as i64 + rdh;
        if nw < 0 || nh < 0 || nw > 1 << 16 || nh > 1 << 16 {
            return Err("text region: refined symbol size".into());
        }
        spend(budget, nw as usize * nh as usize)?;
        let data = self.bits.take_bytes(size as usize)?;
        let mut mq = Mq::new(data);
        let gdx = rdw.div_euclid(2) + rdx;
        let gdy = rdh.div_euclid(2) + rdy;
        Ok(Rc::new(decode_refinement(&mut mq, self.gr_cx, nw as usize, nh as usize, self.r_template, base, gdx, gdy, false, self.r_at)?))
    }
}

/// The symbol code table of a text region (7.4.3.1.7): 35 run code lengths, then the length of every symbol's code.
pub(super) fn read_symbol_codes(b: &mut Bits, num_syms: usize) -> R<Table> {
    let mut run_lens = [0u8; 35];
    for l in &mut run_lens {
        *l = b.bits(4)? as u8;
    }
    let runs = Table::of_lengths(&run_lens)?;
    let mut lens: Vec<u8> = Vec::with_capacity(num_syms);
    while lens.len() < num_syms {
        let c = runs.value(b, "run code")?;
        let (len, times) = match c {
            0..=31 => (c as u8, 1usize),
            32 => (*lens.last().ok_or("symbol code lengths repeat a length that does not exist")?, 3 + b.bits(2)? as usize),
            33 => (0, 3 + b.bits(3)? as usize),
            _ => (0, 11 + b.bits(7)? as usize),
        };
        if lens.len() + times > num_syms {
            return Err("symbol code lengths run past the last symbol".into());
        }
        lens.extend(std::iter::repeat_n(len, times));
    }
    b.align();
    Table::of_lengths(&lens)
}

/// Read the selector flags (7.4.3.1.2) into tables.
pub(super) fn text_tables(flags: u16, pick: &mut Picker) -> R<TextTables> {
    let f = flags as u32;
    let fs = pick.pick(f & 3, &[6, 7])?;
    let ds = pick.pick(f >> 2 & 3, &[8, 9, 10])?;
    let dt = pick.pick(f >> 4 & 3, &[11, 12, 13])?;
    let rdw = pick.pick(f >> 6 & 3, &[14, 15])?;
    let rdh = pick.pick(f >> 8 & 3, &[14, 15])?;
    let rdx = pick.pick(f >> 10 & 3, &[14, 15])?;
    let rdy = pick.pick(f >> 12 & 3, &[14, 15])?;
    let rsize = pick.pick(if f >> 14 & 1 == 1 { 3 } else { 0 }, &[1])?;
    Ok(TextTables { fs, ds, dt, rdw, rdh, rdx, rdy, rsize })
}

// ------------------------------------------------------------------------------------------------ symbol dictionary, Huffman

pub(super) struct DictTables {
    pub dh: Rc<Table>,
    pub dw: Rc<Table>,
    pub bmsize: Rc<Table>,
    pub agginst: Rc<Table>,
}

pub(super) fn dict_tables(flags: u16, pick: &mut Picker) -> R<DictTables> {
    let f = flags as u32;
    let dh = pick.pick(f >> 2 & 3, &[4, 5])?;
    let dw = pick.pick(f >> 4 & 3, &[2, 3])?;
    let bmsize = pick.pick(if f >> 6 & 1 == 1 { 3 } else { 0 }, &[1])?;
    let agginst = pick.pick(if f >> 7 & 1 == 1 { 3 } else { 0 }, &[1])?;
    Ok(DictTables { dh, dw, bmsize, agginst })
}

/// The pixels of an uncompressed collective bitmap (rows padded to a byte, 1 is black).
pub(super) fn raw_bitmap(data: &[u8], w: usize, h: usize) -> R<Bitmap> {
    let row = w.div_ceil(8);
    if data.len() < row.checked_mul(h).ok_or("collective bitmap too large")? {
        return Err("collective bitmap is cut short".into());
    }
    let mut bm = Bitmap::new(w, h, 0)?;
    for y in 0..h {
        for x in 0..w {
            bm.d[y * w + x] = (data[y * row + x / 8] >> (7 - x % 8)) & 1;
        }
    }
    Ok(bm)
}

/// What `symbol_dict` needs from the header the arithmetic reader has already parsed.
pub(super) struct DictHeader<'a> {
    pub flags: u16,
    pub refagg: bool,
    pub r_template: u8,
    pub r_at: Vec<(i8, i8)>,
    pub num_new: usize,
    pub gr_cx: Vec<u8>,
    pub custom: &'a [Rc<Table>],
}

/// A Huffman-coded symbol dictionary (6.5.5 with SDHUFF set); returns the exported symbols and the refinement contexts.
pub(super) fn symbol_dict(h: DictHeader, data: &[u8], inputs: &[Rc<Bitmap>], budget: &mut usize) -> R<(Vec<Rc<Bitmap>>, Vec<u8>)> {
    let mut pick = Picker::new(h.custom);
    let t = dict_tables(h.flags, &mut pick)?;
    let agg = TextTables::for_aggregates()?;
    let b1 = standard(1)?;
    let b15 = standard(15)?;
    let mut b = Bits::new(data);
    let mut gr_cx = h.gr_cx;
    let total = inputs.len() + h.num_new;
    let sym_len = super::code_len(total).max(1);
    let codes = SymCodes::Fixed(sym_len);
    let mut news: Vec<Rc<Bitmap>> = Vec::new();
    let mut height = 0i64;
    while news.len() < h.num_new {
        height += t.dh.value(&mut b, "height")?;
        if !(0..=1 << 16).contains(&height) {
            return Err("symbol dictionary: symbol height".into());
        }
        let mut widths: Vec<usize> = Vec::new();
        let mut width = 0i64;
        let mut tot = 0usize;
        loop {
            let Some(d) = t.dw.decode(&mut b)? else { break };
            if news.len() + widths.len() >= h.num_new {
                return Err("symbol dictionary: more symbols than announced".into());
            }
            width += d;
            if !(0..=1 << 16).contains(&width) {
                return Err("symbol dictionary: symbol width".into());
            }
            let (sw, sh) = (width as usize, height as usize);
            if !h.refagg {
                widths.push(sw);
                tot += sw;
                continue;
            }
            spend(budget, sw * sh)?;
            let ninst = t.agginst.value(&mut b, "instance count")?;
            let bm = if ninst > 1 {
                let all: Vec<Rc<Bitmap>> = inputs.iter().chain(news.iter()).cloned().collect();
                let params = TextParams { refine: true, log_strips: 0, ref_corner: 1, transposed: false, comb_op: 0, def_pixel: 0, ds_offset: 0, r_at: h.r_at.clone(), num_instances: ninst as u32 };
                let mut src = HuffText { bits: &mut b, t: &agg, codes: &codes, log_strips: 0, refine: true, gr_cx: &mut gr_cx, r_template: h.r_template, r_at: &h.r_at };
                super::decode_text(&mut src, sw, sh, &params, &all, budget)?
            } else {
                let id = b.bits(sym_len)? as usize;
                let rdx = b15.value(&mut b, "dx")?;
                let rdy = b15.value(&mut b, "dy")?;
                let size = b1.value(&mut b, "size")?;
                if size < 0 {
                    return Err("refinement size below zero".into());
                }
                let base = inputs.iter().chain(news.iter()).nth(id).ok_or("symbol dictionary: refined symbol does not exist")?.clone();
                let bytes = b.take_bytes(size as usize)?;
                let mut mq = Mq::new(bytes);
                decode_refinement(&mut mq, &mut gr_cx, sw, sh, h.r_template, &base, rdx, rdy, false, &h.r_at)?
            };
            news.push(Rc::new(bm));
        }
        if !h.refagg {
            let size = t.bmsize.value(&mut b, "bitmap size")?;
            if size < 0 {
                return Err("collective bitmap size below zero".into());
            }
            let sh = height as usize;
            let all = Bitmap::new(tot, sh, 0)?;
            spend(budget, all.d.len())?;
            let bm = if size == 0 {
                let bytes = b.take_bytes(tot.div_ceil(8) * sh)?;
                raw_bitmap(bytes, tot, sh)?
            } else {
                let bytes = b.take_bytes(size as usize)?;
                super::mmr_bitmap_of(bytes, tot, sh)?
            };
            let mut x = 0usize;
            for w in widths {
                news.push(Rc::new(bm.sub(x as i64, 0, w, sh)?));
                x += w;
            }
        }
    }
    let mut flags_ex = vec![false; total];
    let (mut i, mut cur) = (0usize, false);
    while i < total {
        let run = b1.value(&mut b, "export run")?;
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
    Ok((exported, gr_cx))
}
