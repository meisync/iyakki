//! CCITT Group 3 (one and two dimensional) and Group 4 fax decoding (ITU-T T.4 and T.6), written in-house, for the `CCITTFaxDecode`
//! filter of scanned PDF pages. The code tables are the standard's; the decoder reads a row at a time, either as runs of white and
//! black (one dimensional) or relative to the row above (two dimensional: pass, horizontal and the six vertical modes). A damaged
//! stream gives the rows read so far, and the rest of the page is white.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The parameters of the filter, as the PDF names them.
#[derive(Clone, Debug)]
pub struct Params {
    /// Below 0: Group 4. 0: Group 3 one dimensional. Above 0: Group 3 mixed (a tag bit after each end-of-line says which).
    pub k: i64,
    pub columns: usize,
    /// The number of rows if known (0: until the data ends).
    pub rows: usize,
    pub black_is_1: bool,
    pub byte_align: bool,
}

impl Default for Params {
    fn default() -> Params {
        Params { k: 0, columns: 1728, rows: 0, black_is_1: false, byte_align: false }
    }
}

const WHITE_TERM: [&str; 64] = [
    "00110101", "000111", "0111", "1000", "1011", "1100", "1110", "1111", "10011", "10100", "00111", "01000", "001000", "000011", "110100", "110101", "101010",
    "101011", "0100111", "0001100", "0001000", "0010111", "0000011", "0000100", "0101000", "0101011", "0010011", "0100100", "0011000", "00000010", "00000011",
    "00011010", "00011011", "00010010", "00010011", "00010100", "00010101", "00010110", "00010111", "00101000", "00101001", "00101010", "00101011", "00101100",
    "00101101", "00000100", "00000101", "00001010", "00001011", "01010010", "01010011", "01010100", "01010101", "00100100", "00100101", "01011000", "01011001",
    "01011010", "01011011", "01001010", "01001011", "00110010", "00110011", "00110100",
];
const WHITE_MAKEUP: [&str; 27] = [
    "11011", "10010", "010111", "0110111", "00110110", "00110111", "01100100", "01100101", "01101000", "01100111", "011001100", "011001101", "011010010",
    "011010011", "011010100", "011010101", "011010110", "011010111", "011011000", "011011001", "011011010", "011011011", "010011000", "010011001", "010011010",
    "011000", "010011011",
];
const BLACK_TERM: [&str; 64] = [
    "0000110111", "010", "11", "10", "011", "0011", "0010", "00011", "000101", "000100", "0000100", "0000101", "0000111", "00000100", "00000111", "000011000",
    "0000010111", "0000011000", "0000001000", "00001100111", "00001101000", "00001101100", "00000110111", "00000101000", "00000010111", "00000011000",
    "000011001010", "000011001011", "000011001100", "000011001101", "000001101000", "000001101001", "000001101010", "000001101011", "000011010010",
    "000011010011", "000011010100", "000011010101", "000011010110", "000011010111", "000001101100", "000001101101", "000011011010", "000011011011",
    "000001010100", "000001010101", "000001010110", "000001010111", "000001100100", "000001100101", "000001010010", "000001010011", "000000100100",
    "000000110111", "000000111000", "000000100111", "000000101000", "000001011000", "000001011001", "000000101011", "000000101100", "000001011010",
    "000001100110", "000001100111",
];
const BLACK_MAKEUP: [&str; 27] = [
    "0000001111", "000011001000", "000011001001", "000001011011", "000000110011", "000000110100", "000000110101", "0000001101100", "0000001101101",
    "0000001001010", "0000001001011", "0000001001100", "0000001001101", "0000001110010", "0000001110011", "0000001110100", "0000001110101", "0000001110110",
    "0000001110111", "0000001010010", "0000001010011", "0000001010100", "0000001010101", "0000001011010", "0000001011011", "0000001100100", "0000001100101",
];
/// The make-up codes 1792 to 2560 shared by both colours.
const EXT_MAKEUP: [&str; 13] = [
    "00000001000", "00000001100", "00000001101", "000000010010", "000000010011", "000000010100", "000000010101", "000000010110", "000000010111", "000000011100",
    "000000011101", "000000011110", "000000011111",
];

type Table = HashMap<(u8, u16), u16>;

fn build(term: &[&str], makeup: &[&str]) -> Table {
    let mut t = Table::new();
    let put = |t: &mut Table, code: &str, run: u16| {
        t.insert((code.len() as u8, u16::from_str_radix(code, 2).unwrap()), run);
    };
    for (i, c) in term.iter().enumerate() {
        put(&mut t, c, i as u16);
    }
    for (i, c) in makeup.iter().enumerate() {
        put(&mut t, c, 64 * (i as u16 + 1));
    }
    for (i, c) in EXT_MAKEUP.iter().enumerate() {
        put(&mut t, c, 1792 + 64 * i as u16);
    }
    t
}

fn tables() -> &'static (Table, Table) {
    static T: OnceLock<(Table, Table)> = OnceLock::new();
    T.get_or_init(|| (build(&WHITE_TERM, &WHITE_MAKEUP), build(&BLACK_TERM, &BLACK_MAKEUP)))
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn left(&self) -> usize {
        (self.d.len() * 8).saturating_sub(self.pos)
    }
    fn bit(&mut self) -> Option<u32> {
        let b = *self.d.get(self.pos / 8)?;
        let v = (b >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(v as u32)
    }
    fn peek(&self, n: usize) -> u32 {
        let mut v = 0;
        for i in 0..n {
            let p = self.pos + i;
            let bit = self.d.get(p / 8).map(|b| (b >> (7 - p % 8)) & 1).unwrap_or(0);
            v = (v << 1) | bit as u32;
        }
        v
    }
    fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }
}

/// A run of one colour: the make-up codes and the ending code added up. None at damage or the end of the data.
fn read_run(b: &mut Bits, black: bool) -> Option<usize> {
    let t = tables();
    let table = if black { &t.1 } else { &t.0 };
    let mut total = 0usize;
    loop {
        let mut code = 0u16;
        let mut found = None;
        for len in 1..=13u8 {
            code = (code << 1) | b.bit()? as u16;
            if let Some(&r) = table.get(&(len, code)) {
                found = Some(r as usize);
                break;
            }
        }
        let r = found?;
        total += r;
        if r < 64 {
            return Some(total);
        }
        if total > 1 << 20 {
            return None;
        }
    }
}

#[derive(PartialEq)]
enum Mode {
    Pass,
    Horizontal,
    Vertical(i32),
    Eol,
    Bad,
}

fn read_mode(b: &mut Bits) -> Mode {
    let Some(first) = b.bit() else { return Mode::Eol };
    if first == 1 {
        return Mode::Vertical(0);
    }
    let mut code = 0u32;
    // after the leading 0
    for len in 2..=7 {
        let Some(bit) = b.bit() else { return Mode::Eol };
        code = (code << 1) | bit;
        match (len, code) {
            (3, 0b11) => return Mode::Vertical(1),
            (3, 0b10) => return Mode::Vertical(-1),
            (3, 0b01) => return Mode::Horizontal,
            (4, 0b001) => return Mode::Pass,
            (6, 0b0011) => return Mode::Vertical(2),
            (6, 0b0010) => return Mode::Vertical(-2),
            (7, 0b00011) => return Mode::Vertical(3),
            (7, 0b00010) => return Mode::Vertical(-3),
            _ => {}
        }
    }
    // 0000001 starts an extension, 0000000 an end-of-line (or fill)
    if code == 0 {
        Mode::Eol
    } else {
        Mode::Bad
    }
}

/// Decode a row relative to the reference row. `refl` and the result are lists of the changing elements (the columns where the colour
/// changes, starting from white); the result has an even number of entries.
fn row_2d(b: &mut Bits, refl: &[usize], cols: usize) -> Result<Vec<usize>, Mode> {
    let mut cur: Vec<usize> = Vec::new();
    let mut a0: isize = -1;
    let mut bi = 0usize;
    while (a0 as i64) < cols as i64 {
        let color = cur.len() % 2;
        // b1: the first changing element right of a0 whose change is to the colour opposite to a0's
        bi = bi.saturating_sub(2);
        while bi < refl.len() && (refl[bi] as isize <= a0 || bi % 2 != color) {
            bi += 1;
        }
        let b1 = refl.get(bi).copied().unwrap_or(cols);
        let b2 = refl.get(bi + 1).copied().unwrap_or(cols);
        match read_mode(b) {
            Mode::Pass => a0 = b2 as isize,
            Mode::Horizontal => {
                let r1 = read_run(b, color == 1).ok_or(Mode::Bad)?;
                let r2 = read_run(b, color == 0).ok_or(Mode::Bad)?;
                let s = a0.max(0) as usize;
                let a1 = (s + r1).min(cols);
                let a2 = (a1 + r2).min(cols);
                cur.push(a1);
                cur.push(a2);
                a0 = a2 as isize;
            }
            Mode::Vertical(d) => {
                let a1 = (b1 as isize + d as isize).clamp(a0.max(0), cols as isize) as usize;
                cur.push(a1);
                a0 = a1 as isize;
            }
            m => return Err(m),
        }
    }
    Ok(cur)
}

fn row_1d(b: &mut Bits, cols: usize) -> Option<Vec<usize>> {
    let mut cur = Vec::new();
    let mut x = 0usize;
    let mut black = false;
    while x < cols {
        let r = read_run(b, black)?;
        x = (x + r).min(cols);
        cur.push(x);
        black = !black;
    }
    Some(cur)
}

/// Skip fill bits and an end-of-line if one is next; true if one was there.
fn skip_eol(b: &mut Bits) -> bool {
    let save = b.pos;
    let mut zeros = 0;
    while b.left() > 0 && b.peek(1) == 0 {
        b.pos += 1;
        zeros += 1;
        if zeros > 64 {
            break;
        }
    }
    if zeros >= 11 && b.peek(1) == 1 {
        b.pos += 1;
        true
    } else {
        b.pos = save;
        false
    }
}

/// Decode to packed rows of one bit per pixel, white 1 and black 0 (or the other way with `black_is_1`), each row padded to a byte.
/// Returns the rows and how many were really decoded (the rest are white).
pub fn decode(data: &[u8], p: &Params, max_pixels: usize) -> (Vec<u8>, usize) {
    let (out, rows, _) = decode_counting(data, p, max_pixels);
    (out, rows)
}

/// As [`decode`], and also the number of bytes the picture took: after the last row the data is taken to the next byte boundary, and an
/// end-of-facsimile-block (EOFB, two end-of-line codes) is taken with it when one follows (JBIG2 MMR bit planes lie one after the other).
pub fn decode_counting(data: &[u8], p: &Params, max_pixels: usize) -> (Vec<u8>, usize, usize) {
    let cols = p.columns.clamp(1, 1 << 16);
    let row_bytes = cols.div_ceil(8);
    let max_rows = if p.rows > 0 { p.rows } else { (max_pixels / cols).min(1 << 16) };
    let mut b = Bits { d: data, pos: 0 };
    let mut out: Vec<u8> = Vec::new();
    let mut refl: Vec<usize> = Vec::new();
    let mut rows = 0usize;
    while rows < max_rows && b.left() > 0 {
        if p.byte_align {
            b.align();
        }
        let cur: Option<Vec<usize>>;
        if p.k < 0 {
            // a Group 4 stream ends with two end-of-line codes
            if b.peek(12) == 1 {
                break;
            }
            cur = row_2d(&mut b, &refl, cols).ok();
        } else {
            let had_eol = skip_eol(&mut b);
            // several end-of-lines in a row end the data (Group 3 return to control)
            if had_eol && b.peek(12) == 1 {
                break;
            }
            let one_d = if p.k > 0 && had_eol { b.bit().unwrap_or(1) == 1 } else { p.k == 0 || !had_eol };
            cur = if one_d { row_1d(&mut b, cols) } else { row_2d(&mut b, &refl, cols).ok() };
        }
        let Some(cur) = cur else { break };
        // paint the row
        let start = out.len();
        out.resize(start + row_bytes, 0);
        let mut x = 0usize;
        let mut black = false;
        let mut ends = cur.clone();
        if ends.last().copied().unwrap_or(0) < cols {
            ends.push(cols);
        }
        for &e in &ends {
            let e = e.min(cols);
            if black == p.black_is_1 {
                // this colour is stored as 1
                for xx in x..e {
                    out[start + xx / 8] |= 0x80 >> (xx % 8);
                }
            }
            x = e.max(x);
            black = !black;
        }
        // the changing elements of this row, without the end marker the painting added
        refl = cur;
        rows += 1;
    }
    let total = if p.rows > 0 { p.rows } else { rows };
    let fill_white: u8 = if p.black_is_1 { 0x00 } else { 0xFF };
    let have = out.len();
    out.resize(total * row_bytes, fill_white);
    let _ = have;
    if p.k < 0 && b.left() >= 24 && b.peek(24) == 0x001001 {
        b.pos += 24;
    }
    b.align();
    (out, rows, (b.pos / 8).min(data.len()))
}
