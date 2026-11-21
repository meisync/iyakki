// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! A Brotli decoder (RFC 7932), written here from the format description (F-BROX-19). It exists to open WOFF2 web fonts (`woff2.rs`), whose
//! tables are one Brotli stream. It reads everything the format has: simple and complex prefix codes, block types and counts, context maps
//! with their move-to-front, the three context modes, the distance ring and the direct and postfix distance codes, uncompressed and metadata
//! meta-blocks, and the static dictionary with its 121 transforms (`tables.rs`, `dictionary.bin`).
//!
//! Hostile input: every length is checked before it is used, the output is capped by the caller (`max_out`), a distance beyond what has
//! been written is a dictionary reference only inside the dictionary's range, and nothing here panics on any bytes (a test feeds noise).
//! Large-window streams (a Brotli extension) are refused.

mod tables;

use tables::{CONTEXT_LUT, TRANSFORMS};

static DICTIONARY: &[u8] = include_bytes!("dictionary.bin");

/// `NDBITS` of the dictionary words of length 0..=24 (the number of words is `1 << bits`).
const DICT_BITS: [u8; 25] = [0, 0, 0, 0, 10, 10, 11, 11, 10, 10, 10, 10, 10, 9, 9, 8, 7, 7, 8, 7, 7, 6, 6, 5, 5];

const INSERT_BASE: [u32; 24] = [0, 1, 2, 3, 4, 5, 6, 8, 10, 14, 18, 26, 34, 50, 66, 98, 130, 194, 322, 578, 1090, 2114, 6210, 22594];
const INSERT_EXTRA: [u8; 24] = [0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 12, 14, 24];
const COPY_BASE: [u32; 24] = [2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 14, 18, 22, 30, 38, 54, 70, 102, 134, 198, 326, 582, 1094, 2118];
const COPY_EXTRA: [u8; 24] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 24];
const BLOCK_BASE: [u32; 26] = [1, 5, 9, 13, 17, 25, 33, 41, 49, 65, 81, 97, 113, 145, 177, 209, 241, 305, 369, 497, 753, 1265, 2289, 4337, 8433, 16625];
const BLOCK_EXTRA: [u8; 26] = [2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 7, 8, 9, 10, 11, 12, 13, 24];
/// (insert range start, copy range start) of the 11 cells of 64 command symbols.
const CELLS: [(u8, u8); 11] = [(0, 0), (0, 8), (0, 0), (0, 8), (8, 0), (8, 8), (0, 16), (16, 0), (8, 16), (16, 8), (16, 16)];
const CODE_LENGTH_ORDER: [usize; 18] = [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];
/// The fixed code of the code lengths of the code lengths, looked up by the next four bits: (length, value).
const CL_CODE: [(u8, u8); 16] = [(2, 0), (2, 4), (2, 3), (3, 2), (2, 0), (2, 4), (2, 3), (4, 1), (2, 0), (2, 4), (2, 3), (3, 2), (2, 0), (2, 4), (2, 3), (4, 5)];

#[derive(Debug, PartialEq, Eq)]
pub enum BrotliError {
    /// The stream is cut short.
    Truncated,
    /// The stream breaks a rule of the format; the text says which.
    Invalid(&'static str),
    /// The answer would be bigger than the caller allows.
    TooBig,
}

type R<T> = Result<T, BrotliError>;

struct Bits<'a> {
    d: &'a [u8],
    /// Bit position from the start.
    pos: usize,
}

impl<'a> Bits<'a> {
    fn bit(&mut self) -> R<u32> {
        let byte = *self.d.get(self.pos >> 3).ok_or(BrotliError::Truncated)?;
        let b = u32::from(byte >> (self.pos & 7)) & 1;
        self.pos += 1;
        Ok(b)
    }

    /// `n` bits (0 to 24), the first one read is the lowest.
    fn bits(&mut self, n: u32) -> R<u32> {
        let mut v = 0;
        for i in 0..n {
            v |= self.bit()? << i;
        }
        Ok(v)
    }

    fn peek4(&self) -> u32 {
        let mut v = 0;
        for i in 0..4 {
            let p = self.pos + i;
            let b = self.d.get(p >> 3).map_or(0, |byte| u32::from(byte >> (p & 7)) & 1);
            v |= b << i;
        }
        v
    }

    fn align(&mut self) -> R<()> {
        while self.pos & 7 != 0 {
            if self.bit()? != 0 {
                return Err(BrotliError::Invalid("padding bits are not zero"));
            }
        }
        Ok(())
    }
}

/// A canonical prefix code, read bit by bit the way the format packs it (the first bit is the top bit of the code).
#[derive(Clone, Default)]
struct Code {
    /// How many codes there are of each length 1..=15.
    count: [u16; 16],
    /// The symbols sorted by (length, symbol).
    symbols: Vec<u16>,
    /// A code of one symbol takes no bits.
    single: Option<u16>,
}

impl Code {
    fn from_lengths(lengths: &[u8]) -> R<Code> {
        let mut count = [0u16; 16];
        for &l in lengths {
            count[usize::from(l)] += 1;
        }
        count[0] = 0;
        let mut offs = [0u16; 17];
        for l in 1..16 {
            offs[l + 1] = offs[l] + count[l];
        }
        let total = offs[16] as usize;
        let mut symbols = vec![0u16; total];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[usize::from(offs[usize::from(l)])] = s as u16;
                offs[usize::from(l)] += 1;
            }
        }
        Ok(Code { count, symbols, single: None })
    }

    fn one(symbol: u16) -> Code {
        Code { count: [0; 16], symbols: vec![symbol], single: Some(symbol) }
    }

    fn decode(&self, b: &mut Bits) -> R<u16> {
        if let Some(s) = self.single {
            return Ok(s);
        }
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.bit()? as i32;
            let count = i32::from(self.count[len]);
            if code - count < first {
                return self.symbols.get((index + (code - first)) as usize).copied().ok_or(BrotliError::Invalid("bad prefix code"));
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(BrotliError::Invalid("bad prefix code"))
    }
}

fn alphabet_bits(size: usize) -> u32 {
    let mut n = 0;
    while (1usize << n) < size {
        n += 1;
    }
    n
}

/// A prefix code over `alphabet` symbols (RFC 7932, section 3.4).
fn read_code(b: &mut Bits, alphabet: usize) -> R<Code> {
    let hskip = b.bits(2)?;
    if hskip == 1 {
        let nsym = b.bits(2)? as usize + 1;
        let abits = alphabet_bits(alphabet);
        let mut syms = Vec::with_capacity(4);
        for _ in 0..nsym {
            let s = b.bits(abits)? as usize;
            if s >= alphabet {
                return Err(BrotliError::Invalid("symbol outside the alphabet"));
            }
            if syms.contains(&(s as u16)) {
                return Err(BrotliError::Invalid("a symbol is listed twice"));
            }
            syms.push(s as u16);
        }
        let mut lengths = vec![0u8; alphabet];
        match nsym {
            1 => return Ok(Code::one(syms[0])),
            2 => {
                lengths[usize::from(syms[0])] = 1;
                lengths[usize::from(syms[1])] = 1;
            }
            3 => {
                if syms[1] > syms[2] {
                    syms.swap(1, 2);
                }
                lengths[usize::from(syms[0])] = 1;
                lengths[usize::from(syms[1])] = 2;
                lengths[usize::from(syms[2])] = 2;
            }
            _ => {
                if b.bit()? == 0 {
                    for s in &syms {
                        lengths[usize::from(*s)] = 2;
                    }
                } else {
                    if syms[2] > syms[3] {
                        syms.swap(2, 3);
                    }
                    for (s, l) in syms.iter().zip([1u8, 2, 3, 3]) {
                        lengths[usize::from(*s)] = l;
                    }
                }
            }
        }
        return Code::from_lengths(&lengths);
    }
    // a complex code: the lengths of the code length codes first
    let mut cl_len = [0u8; 18];
    let (mut space, mut num_codes) = (32i32, 0);
    for &i in &CODE_LENGTH_ORDER[hskip as usize..] {
        let (len, val) = CL_CODE[b.peek4() as usize];
        b.pos += usize::from(len);
        if b.pos > b.d.len() * 8 {
            return Err(BrotliError::Truncated);
        }
        cl_len[i] = val;
        if val != 0 {
            space -= 32 >> val;
            num_codes += 1;
            if space <= 0 {
                break;
            }
        }
    }
    if !(num_codes == 1 || space == 0) {
        return Err(BrotliError::Invalid("the code length code is not complete"));
    }
    let cl_code = if num_codes == 1 {
        let s = cl_len.iter().position(|&l| l != 0).unwrap_or(0);
        Code::one(s as u16)
    } else {
        Code::from_lengths(&cl_len)?
    };
    // then the lengths of the symbols
    let mut lengths = vec![0u8; alphabet];
    let (mut sym, mut space, mut prev, mut repeat, mut repeat_len) = (0usize, 32768i32, 8u8, 0u32, 0u8);
    while sym < alphabet && space > 0 {
        let c = cl_code.decode(b)? as u8;
        if c < 16 {
            repeat = 0;
            lengths[sym] = c;
            sym += 1;
            if c != 0 {
                prev = c;
                space -= 32768 >> c;
            }
        } else {
            let extra = if c == 16 { 2 } else { 3 };
            let new_len = if c == 16 { prev } else { 0 };
            if repeat_len != new_len {
                repeat = 0;
                repeat_len = new_len;
            }
            let old = repeat;
            if repeat > 0 {
                repeat -= 2;
                repeat <<= extra;
            }
            repeat += b.bits(extra)? + 3;
            let delta = (repeat - old) as usize;
            if sym + delta > alphabet {
                return Err(BrotliError::Invalid("a run of code lengths is too long"));
            }
            for l in &mut lengths[sym..sym + delta] {
                *l = new_len;
            }
            sym += delta;
            if new_len != 0 {
                prev = new_len;
                space -= (delta as i32) << (15 - new_len);
            }
        }
    }
    if space != 0 {
        return Err(BrotliError::Invalid("the prefix code is not complete"));
    }
    Code::from_lengths(&lengths)
}

/// 1 to 256 (RFC 7932, section 9.2).
fn var_len_256(b: &mut Bits) -> R<usize> {
    if b.bit()? == 0 {
        return Ok(1);
    }
    let n = b.bits(3)?;
    if n == 0 {
        return Ok(2);
    }
    Ok(((1u32 << n) + b.bits(n)? + 1) as usize)
}

struct Kind {
    n: usize,
    code: Option<Code>,
    count_code: Option<Code>,
    /// The last two block types: `[second last, last]`.
    types: [usize; 2],
    left: u32,
    cur: usize,
}

impl Kind {
    fn read(b: &mut Bits) -> R<Kind> {
        let n = var_len_256(b)?;
        let mut k = Kind { n, code: None, count_code: None, types: [1, 0], left: 1 << 24, cur: 0 };
        if n >= 2 {
            k.code = Some(read_code(b, n + 2)?);
            k.count_code = Some(read_code(b, 26)?);
            k.left = k.next_count(b)?;
        }
        Ok(k)
    }

    fn next_count(&self, b: &mut Bits) -> R<u32> {
        let s = self.count_code.as_ref().ok_or(BrotliError::Invalid("no block count code"))?.decode(b)? as usize;
        Ok(BLOCK_BASE[s] + b.bits(u32::from(BLOCK_EXTRA[s]))?)
    }

    /// Called before every symbol of this kind: a new block starts when the last one is used up.
    fn step(&mut self, b: &mut Bits) -> R<()> {
        if self.left == 0 {
            let s = self.code.as_ref().ok_or(BrotliError::Invalid("a block ended in a stream with one block"))?.decode(b)? as usize;
            let t = match s {
                0 => self.types[0],
                1 => {
                    let t = self.types[1] + 1;
                    if t >= self.n { t - self.n } else { t }
                }
                s => s - 2,
            };
            if t >= self.n {
                return Err(BrotliError::Invalid("a block type is out of range"));
            }
            self.types = [self.types[1], t];
            self.cur = t;
            self.left = self.next_count(b)?;
        }
        self.left -= 1;
        Ok(())
    }
}

/// A context map of `size` entries into `trees` trees (RFC 7932, section 7.3).
fn read_context_map(b: &mut Bits, size: usize) -> R<(Vec<u8>, usize)> {
    let trees = var_len_256(b)?;
    if trees == 1 {
        return Ok((vec![0; size], 1));
    }
    let rle_max = if b.bit()? == 1 { b.bits(4)? as usize + 1 } else { 0 };
    let code = read_code(b, trees + rle_max)?;
    let mut map = vec![0u8; size];
    let mut i = 0;
    while i < size {
        let s = code.decode(b)? as usize;
        if s == 0 {
            i += 1;
        } else if s <= rle_max {
            let reps = (1usize << s) + b.bits(s as u32)? as usize;
            if i + reps > size {
                return Err(BrotliError::Invalid("a run in a context map is too long"));
            }
            i += reps;
        } else {
            map[i] = (s - rle_max) as u8;
            i += 1;
        }
    }
    if b.bit()? == 1 {
        let mut mtf: Vec<u8> = (0..=255).collect();
        for v in map.iter_mut() {
            let idx = usize::from(*v);
            let val = mtf[idx];
            mtf.copy_within(0..idx, 1);
            mtf[0] = val;
            *v = val;
        }
    }
    if map.iter().any(|&v| usize::from(v) >= trees) {
        return Err(BrotliError::Invalid("a context map names a tree that is not there"));
    }
    Ok((map, trees))
}

fn upper(p: &mut [u8]) -> usize {
    if p[0] < 0xc0 {
        if p[0].is_ascii_lowercase() {
            p[0] ^= 32;
        }
        1
    } else if p[0] < 0xe0 {
        if p.len() > 1 {
            p[1] ^= 32;
        }
        2
    } else {
        if p.len() > 2 {
            p[2] ^= 5;
        }
        3
    }
}

/// The dictionary word `word_id` of length `len`, put through transform `t`.
fn dictionary_word(len: usize, word_id: usize, out: &mut Vec<u8>) -> R<()> {
    if !(4..=24).contains(&len) {
        return Err(BrotliError::Invalid("a dictionary word of that length does not exist"));
    }
    let bits = u32::from(DICT_BITS[len]);
    let offset: usize = (0..len).map(|l| if l >= 4 { (1usize << DICT_BITS[l]) * l } else { 0 }).sum();
    let t = word_id >> bits;
    let (prefix, kind, suffix) = *TRANSFORMS.get(t).ok_or(BrotliError::Invalid("a dictionary transform that does not exist"))?;
    let start = offset + (word_id & ((1 << bits) - 1)) * len;
    let mut word = DICTIONARY.get(start..start + len).ok_or(BrotliError::Invalid("a dictionary word outside the dictionary"))?.to_vec();
    match kind {
        0 => {}
        1..=9 => word.truncate(len.saturating_sub(usize::from(kind))),
        10 => {
            if !word.is_empty() {
                upper(&mut word);
            }
        }
        11 => {
            let mut i = 0;
            while i < word.len() {
                i += upper(&mut word[i..]);
            }
        }
        _ => {
            let skip = usize::from(kind - 11).min(word.len());
            word.drain(..skip);
        }
    }
    out.extend_from_slice(prefix);
    out.extend_from_slice(&word);
    out.extend_from_slice(suffix);
    Ok(())
}

/// Decode a Brotli stream; the answer may be at most `max_out` bytes.
pub fn decompress(input: &[u8], max_out: usize) -> Result<Vec<u8>, BrotliError> {
    let mut b = Bits { d: input, pos: 0 };
    // the window size
    let wbits = if b.bit()? == 0 {
        16
    } else {
        let n = b.bits(3)?;
        if n != 0 {
            17 + n
        } else {
            match b.bits(3)? {
                0 => 17,
                1 => return Err(BrotliError::Invalid("large windows are not supported")),
                m => 8 + m,
            }
        }
    };
    let window = (1usize << wbits) - 16;
    let mut out: Vec<u8> = Vec::new();
    let mut ring: [i64; 4] = [16, 15, 11, 4];
    let mut ring_at = 0usize;
    loop {
        let last = b.bit()? == 1;
        if last && b.bit()? == 1 {
            break;
        }
        let nibbles = match b.bits(2)? {
            0 => 4,
            1 => 5,
            2 => 6,
            _ => 0,
        };
        if nibbles == 0 {
            // metadata: skipped
            if b.bit()? != 0 {
                return Err(BrotliError::Invalid("a reserved bit is set"));
            }
            let nbytes = b.bits(2)? as usize;
            let mut skip = 0usize;
            for i in 0..nbytes {
                skip |= (b.bits(8)? as usize) << (8 * i);
            }
            if nbytes > 0 {
                skip += 1;
            }
            b.align()?;
            let at = b.pos / 8;
            if at + skip > input.len() {
                return Err(BrotliError::Truncated);
            }
            b.pos += skip * 8;
            if last {
                break;
            }
            continue;
        }
        let coded = b.bits(nibbles * 4)? as usize;
        if nibbles > 4 && coded >> ((nibbles - 1) * 4) == 0 {
            return Err(BrotliError::Invalid("a length is written with too many digits"));
        }
        let mlen = coded + 1;
        if out.len() + mlen > max_out {
            return Err(BrotliError::TooBig);
        }
        if !last && b.bit()? == 1 {
            b.align()?;
            let at = b.pos / 8;
            let chunk = input.get(at..at + mlen).ok_or(BrotliError::Truncated)?;
            out.extend_from_slice(chunk);
            b.pos += mlen * 8;
            continue;
        }
        let end = out.len() + mlen;
        let mut lit = Kind::read(&mut b)?;
        let mut cmd = Kind::read(&mut b)?;
        let mut dist = Kind::read(&mut b)?;
        let npostfix = b.bits(2)?;
        let ndirect = (b.bits(4)? as usize) << npostfix;
        let modes: Vec<u8> = (0..lit.n).map(|_| b.bits(2).map(|v| v as u8)).collect::<R<_>>()?;
        let (lit_map, lit_trees) = read_context_map(&mut b, 64 * lit.n)?;
        let (dist_map, dist_trees) = read_context_map(&mut b, 4 * dist.n)?;
        let lit_codes: Vec<Code> = (0..lit_trees).map(|_| read_code(&mut b, 256)).collect::<R<_>>()?;
        let cmd_codes: Vec<Code> = (0..cmd.n).map(|_| read_code(&mut b, 704)).collect::<R<_>>()?;
        let dist_alphabet = 16 + ndirect + (48usize << npostfix);
        let dist_codes: Vec<Code> = (0..dist_trees).map(|_| read_code(&mut b, dist_alphabet)).collect::<R<_>>()?;
        while out.len() < end {
            cmd.step(&mut b)?;
            let sym = usize::from(cmd_codes[cmd.cur].decode(&mut b)?);
            let (ins_start, copy_start) = CELLS[sym >> 6];
            let ins_code = usize::from(ins_start) + ((sym >> 3) & 7);
            let copy_code = usize::from(copy_start) + (sym & 7);
            let ins_len = (INSERT_BASE[ins_code] + b.bits(u32::from(INSERT_EXTRA[ins_code]))?) as usize;
            let copy_len = (COPY_BASE[copy_code] + b.bits(u32::from(COPY_EXTRA[copy_code]))?) as usize;
            if ins_len > end - out.len() {
                return Err(BrotliError::Invalid("more literals than the meta-block has room for"));
            }
            for _ in 0..ins_len {
                lit.step(&mut b)?;
                let n = out.len();
                let (p1, p2) = (if n >= 1 { usize::from(out[n - 1]) } else { 0 }, if n >= 2 { usize::from(out[n - 2]) } else { 0 });
                let base = usize::from(modes[lit.cur]) << 9;
                let ctx = usize::from(CONTEXT_LUT[base + p1] | CONTEXT_LUT[base + 256 + p2]);
                let tree = usize::from(lit_map[64 * lit.cur + ctx]);
                out.push(lit_codes[tree].decode(&mut b)? as u8);
            }
            if out.len() >= end {
                break;
            }
            // the distance
            let mut implicit_zero = sym < 128;
            let mut dcode = 0usize;
            if !implicit_zero {
                dist.step(&mut b)?;
                let ctx = if copy_len > 4 { 3 } else { copy_len - 2 };
                let tree = usize::from(dist_map[4 * dist.cur + ctx]);
                dcode = usize::from(dist_codes[tree].decode(&mut b)?);
            }
            let distance: i64 = if implicit_zero || dcode == 0 {
                implicit_zero = true;
                ring[(ring_at + 3) & 3]
            } else if dcode < 16 {
                let at = |back: usize| ring[(ring_at + 4 - back) & 3];
                let (base, delta): (i64, i64) = match dcode {
                    1 => (at(2), 0),
                    2 => (at(3), 0),
                    3 => (at(4), 0),
                    4..=9 => (at(1), [-1, 1, -2, 2, -3, 3][dcode - 4]),
                    _ => (at(2), [-1, 1, -2, 2, -3, 3][dcode - 10]),
                };
                base + delta
            } else if dcode < 16 + ndirect {
                (dcode - 15) as i64
            } else {
                let x = dcode - ndirect - 16;
                let nbits = 1 + (x >> (npostfix + 1));
                let hcode = x >> npostfix;
                let lcode = x & ((1 << npostfix) - 1);
                let offset = ((2 + (hcode & 1)) << nbits) - 4;
                let extra = b.bits(nbits as u32)? as usize;
                (((offset + extra) << npostfix) + lcode + ndirect + 1) as i64
            };
            if distance <= 0 {
                return Err(BrotliError::Invalid("a distance of zero or less"));
            }
            let max_distance = out.len().min(window) as i64;
            if distance > max_distance {
                // a word of the static dictionary
                let word_id = (distance - max_distance - 1) as usize;
                let mut word = Vec::new();
                dictionary_word(copy_len, word_id, &mut word)?;
                if word.len() > end - out.len() {
                    return Err(BrotliError::Invalid("a dictionary word runs past the meta-block"));
                }
                out.extend_from_slice(&word);
            } else {
                if !implicit_zero {
                    ring[ring_at & 3] = distance;
                    ring_at += 1;
                }
                if copy_len > end - out.len() {
                    return Err(BrotliError::Invalid("a copy runs past the meta-block"));
                }
                let from = out.len() - distance as usize;
                for i in 0..copy_len {
                    let v = out[from + i];
                    out.push(v);
                }
            }
        }
        if out.len() != end {
            return Err(BrotliError::Invalid("the meta-block is not the length it says"));
        }
        if last {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
