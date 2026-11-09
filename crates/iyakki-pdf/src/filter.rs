//! Stream filters: Flate (own inflate), LZW, ASCII85, ASCIIHex, RunLength, and the PNG/TIFF predictors. Output is capped; a damaged
//! or truncated stream gives what was decoded so far, never a panic.

pub const MAX_STREAM: usize = 96 * 1024 * 1024;

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    acc: u32,
    n: u32,
}

impl<'a> Bits<'a> {
    fn need(&mut self, k: u32) -> Option<()> {
        while self.n < k {
            let b = *self.d.get(self.pos)?;
            self.pos += 1;
            self.acc |= (b as u32) << self.n;
            self.n += 8;
        }
        Some(())
    }
    fn bits(&mut self, k: u32) -> Option<u32> {
        if k == 0 {
            return Some(0);
        }
        self.need(k)?;
        let v = self.acc & ((1u32 << k) - 1);
        self.acc >>= k;
        self.n -= k;
        Some(v)
    }
}

struct Huff {
    count: [u16; 16],
    symbol: Vec<u16>,
}

impl Huff {
    fn new(lengths: &[u8]) -> Huff {
        let mut count = [0u16; 16];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut offs = [0u16; 16];
        for i in 1..15 {
            offs[i + 1] = offs[i] + count[i];
        }
        let mut symbol = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Huff { count, symbol }
    }
    fn decode(&self, b: &mut Bits) -> Option<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.bits(1)? as i32;
            let c = self.count[len] as i32;
            if code - c < first {
                return self.symbol.get((index + (code - first)) as usize).copied();
            }
            index += c;
            first += c;
            first <<= 1;
            code <<= 1;
        }
        None
    }
}

const LBASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEXT: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DBASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DEXT: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// Raw DEFLATE. Returns the output and whether it ended cleanly. Stops at `max`.
pub fn inflate_raw(data: &[u8], max: usize) -> (Vec<u8>, bool) {
    let mut out: Vec<u8> = Vec::new();
    let mut b = Bits { d: data, pos: 0, acc: 0, n: 0 };
    let ok = (|| -> Option<()> {
        loop {
            let last = b.bits(1)?;
            let kind = b.bits(2)?;
            match kind {
                0 => {
                    b.acc = 0;
                    b.n = 0;
                    let len = *b.d.get(b.pos)? as usize | (*b.d.get(b.pos + 1)? as usize) << 8;
                    b.pos += 4;
                    if b.pos > b.d.len() {
                        return None;
                    }
                    let end = (b.pos + len).min(b.d.len());
                    let room = max.saturating_sub(out.len());
                    let take = (end - b.pos.min(end)).min(room);
                    out.extend_from_slice(&b.d[b.pos..b.pos + take]);
                    if end - b.pos < len || take < end - b.pos {
                        return None;
                    }
                    b.pos = end;
                }
                1 | 2 => {
                    let (lit, dist);
                    if kind == 1 {
                        let mut l = [0u8; 288];
                        for (i, x) in l.iter_mut().enumerate() {
                            *x = if i < 144 { 8 } else if i < 256 { 9 } else if i < 280 { 7 } else { 8 };
                        }
                        lit = Huff::new(&l);
                        dist = Huff::new(&[5u8; 30]);
                    } else {
                        let nlen = b.bits(5)? as usize + 257;
                        let ndist = b.bits(5)? as usize + 1;
                        let ncode = b.bits(4)? as usize + 4;
                        if nlen > 286 || ndist > 30 {
                            return None;
                        }
                        const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                        let mut lens = [0u8; 19];
                        for &o in ORDER.iter().take(ncode) {
                            lens[o] = b.bits(3)? as u8;
                        }
                        let cl = Huff::new(&lens);
                        let mut all = vec![0u8; nlen + ndist];
                        let mut i = 0;
                        while i < nlen + ndist {
                            let sym = cl.decode(&mut b)?;
                            if sym < 16 {
                                all[i] = sym as u8;
                                i += 1;
                            } else {
                                let (prev, rep) = match sym {
                                    16 => {
                                        if i == 0 {
                                            return None;
                                        }
                                        (all[i - 1], 3 + b.bits(2)? as usize)
                                    }
                                    17 => (0, 3 + b.bits(3)? as usize),
                                    _ => (0, 11 + b.bits(7)? as usize),
                                };
                                if i + rep > nlen + ndist {
                                    return None;
                                }
                                for _ in 0..rep {
                                    all[i] = prev;
                                    i += 1;
                                }
                            }
                        }
                        lit = Huff::new(&all[..nlen]);
                        dist = Huff::new(&all[nlen..]);
                    }
                    loop {
                        let sym = lit.decode(&mut b)? as usize;
                        if sym < 256 {
                            if out.len() >= max {
                                return None;
                            }
                            out.push(sym as u8);
                        } else if sym == 256 {
                            break;
                        } else {
                            let s = sym - 257;
                            if s >= 29 {
                                return None;
                            }
                            let len = LBASE[s] as usize + b.bits(LEXT[s] as u32)? as usize;
                            let ds = dist.decode(&mut b)? as usize;
                            if ds >= 30 {
                                return None;
                            }
                            let d = DBASE[ds] as usize + b.bits(DEXT[ds] as u32)? as usize;
                            if d > out.len() || out.len() + len > max {
                                return None;
                            }
                            let start = out.len() - d;
                            for k in 0..len {
                                let v = out[start + k];
                                out.push(v);
                            }
                        }
                    }
                }
                _ => return None,
            }
            if last == 1 {
                return Some(());
            }
        }
    })()
    .is_some();
    (out, ok)
}

/// A zlib stream (a two-byte header is skipped when it looks like one; a bare DEFLATE stream is also accepted).
pub fn inflate(data: &[u8], max: usize) -> Vec<u8> {
    let has_header = data.len() >= 2 && data[0] & 0x0f == 8 && ((data[0] as u32) << 8 | data[1] as u32) % 31 == 0;
    let body = if has_header { &data[2..] } else { data };
    let (out, ok) = inflate_raw(body, max);
    if !ok && out.is_empty() && has_header {
        return inflate_raw(data, max).0;
    }
    out
}

pub fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut hi: Option<u8> = None;
    for &c in data {
        let v = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            b'>' => break,
            _ => continue,
        };
        match hi.take() {
            Some(h) => out.push(h * 16 + v),
            None => hi = Some(v),
        }
    }
    if let Some(h) = hi {
        out.push(h * 16);
    }
    out
}

pub fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = [0u32; 5];
    let mut n = 0;
    let mut i = 0;
    while i < data.len() {
        let c = data[i];
        i += 1;
        match c {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                group[n] = (c - b'!') as u32;
                n += 1;
                if n == 5 {
                    let v = group.iter().fold(0u64, |a, &g| a * 85 + g as u64) as u32;
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            _ => {}
        }
        if out.len() > MAX_STREAM {
            break;
        }
    }
    if n > 1 {
        for g in group.iter_mut().skip(n) {
            *g = 84;
        }
        let v = group.iter().fold(0u64, |a, &g| a * 85 + g as u64) as u32;
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    out
}

pub fn run_length(data: &[u8], max: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() && out.len() < max {
        let l = data[i] as usize;
        i += 1;
        if l == 128 {
            break;
        } else if l < 128 {
            let end = (i + l + 1).min(data.len());
            out.extend_from_slice(&data[i..end]);
            i = end;
        } else if let Some(&b) = data.get(i) {
            i += 1;
            out.extend(std::iter::repeat(b).take(257 - l));
        }
    }
    out
}

pub fn lzw(data: &[u8], early: bool, max: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut table: Vec<(u32, u8)> = Vec::new(); // (previous code, byte); codes < 258 are implicit
    let mut width = 9u32;
    let (mut acc, mut n, mut pos) = (0u32, 0u32, 0usize);
    let mut prev: Option<u32> = None;
    let early = early as usize;
    let expand = |table: &Vec<(u32, u8)>, mut code: u32, buf: &mut Vec<u8>| -> Option<u8> {
        buf.clear();
        let mut guard = 0;
        while code >= 258 {
            let (p, b) = *table.get(code as usize - 258)?;
            buf.push(b);
            code = p;
            guard += 1;
            if guard > 5000 {
                return None;
            }
        }
        buf.push(code as u8);
        buf.reverse();
        Some(buf[0])
    };
    let mut buf = Vec::new();
    loop {
        while n < width {
            let Some(&b) = data.get(pos) else { return out };
            pos += 1;
            acc = (acc << 8) | b as u32;
            n += 8;
        }
        let code = (acc >> (n - width)) & ((1 << width) - 1);
        n -= width;
        acc &= (1u32 << n) - 1;
        if code == 256 {
            table.clear();
            width = 9;
            prev = None;
            continue;
        }
        if code == 257 {
            return out;
        }
        let next = 258 + table.len() as u32;
        let first;
        if code < next {
            let Some(f) = expand(&table, code, &mut buf) else { return out };
            first = f;
            out.extend_from_slice(&buf);
        } else if let Some(p) = prev {
            if code != next {
                return out;
            }
            let Some(f) = expand(&table, p, &mut buf) else { return out };
            first = f;
            out.extend_from_slice(&buf);
            out.push(first);
        } else {
            return out;
        }
        if let Some(p) = prev {
            if table.len() < 4096 - 258 {
                table.push((p, first));
            }
        }
        prev = Some(code);
        let sz = 258 + table.len() + early;
        width = if sz >= 2048 { 12 } else if sz >= 1024 { 11 } else if sz >= 512 { 10 } else { 9 };
        if out.len() > max {
            out.truncate(max);
            return out;
        }
    }
}

/// Undo a PNG (10-15) or TIFF (2) predictor.
pub fn predictor(data: Vec<u8>, pred: i64, colors: usize, bpc: usize, columns: usize) -> Vec<u8> {
    let colors = colors.clamp(1, 64);
    let bpc = if matches!(bpc, 1 | 2 | 4 | 8 | 16) { bpc } else { 8 };
    let columns = columns.clamp(1, 1 << 20);
    let bpp = (colors * bpc).div_ceil(8).max(1);
    let row = (colors * bpc * columns).div_ceil(8);
    if pred >= 10 {
        let stride = row + 1;
        let rows = data.len().div_ceil(stride);
        let mut out = vec![0u8; rows * row];
        for r in 0..rows {
            let src = &data[r * stride..((r + 1) * stride).min(data.len())];
            if src.is_empty() {
                break;
            }
            let ft = src[0];
            let (done, cur) = out.split_at_mut(r * row);
            let prev: &[u8] = if r > 0 { &done[(r - 1) * row..] } else { &[] };
            for i in 0..row {
                let x = *src.get(1 + i).unwrap_or(&0);
                let a = if i >= bpp { cur[i - bpp] } else { 0 };
                let b = prev.get(i).copied().unwrap_or(0);
                let c = if i >= bpp { prev.get(i - bpp).copied().unwrap_or(0) } else { 0 };
                cur[i] = match ft {
                    1 => x.wrapping_add(a),
                    2 => x.wrapping_add(b),
                    3 => x.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                    4 => {
                        let (pa, pb, pc) = ((b as i32 - c as i32).abs(), (a as i32 - c as i32).abs(), (a as i32 + b as i32 - 2 * c as i32).abs());
                        let p = if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c };
                        x.wrapping_add(p)
                    }
                    _ => x,
                };
            }
        }
        out
    } else if pred == 2 && bpc == 8 {
        let mut data = data;
        for chunk in data.chunks_mut(row.max(1)) {
            for i in colors..chunk.len() {
                chunk[i] = chunk[i].wrapping_add(chunk[i - colors]);
            }
        }
        data
    } else {
        data
    }
}

/// Deflate with stored blocks only (valid zlib, no compression) and Adler-32; used by the writer.
pub fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    if data.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    let mut chunks = data.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        out.push(if chunks.peek().is_none() { 1 } else { 0 });
        out.extend_from_slice(&(c.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        out.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    out.extend_from_slice(&(b << 16 | a).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_blocks_round_trip_through_inflate() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        assert_eq!(inflate(&zlib_stored(&data), usize::MAX), data);
        assert_eq!(inflate(&zlib_stored(b""), 10), b"");
    }

    #[test]
    fn a_real_fixed_huffman_stream_is_inflated() {
        // zlib.compress(b"hello hello hello hello") at level 9, made once with a reference compressor
        let z = [0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x27, 0x01, 0x68, 0x03, 0x08, 0xb1];
        assert_eq!(inflate(&z, 1000), b"hello hello hello hello");
    }

    #[test]
    fn a_decompression_bomb_is_capped() {
        // fixed-Huffman block of one literal then a long run of max-length matches
        let mut bits: Vec<bool> = vec![true, true, false]; // last, fixed
        let push = |bits: &mut Vec<bool>, v: u32, n: u32| (0..n).rev().for_each(|i| bits.push(v >> i & 1 == 1));
        push(&mut bits, 0x30 + b'a' as u32, 8); // literal 'a'
        for _ in 0..100_000 {
            push(&mut bits, 0b11000101, 8); // length symbol 285 (258)
            push(&mut bits, 0, 5); // distance code 0 (1)
        }
        let out = inflate(&{
            let mut v = vec![0x78, 0x01];
            v.extend(pack(&bits));
            v
        }, 1000);
        assert!(out.len() <= 1000);
    }

    fn pack(bits: &[bool]) -> Vec<u8> {
        let mut out = vec![0u8; bits.len() / 8 + 1];
        for (i, b) in bits.iter().enumerate() {
            if *b {
                out[i / 8] |= 1 << (i % 8);
            }
        }
        out
    }

    #[test]
    fn small_filters() {
        assert_eq!(ascii_hex(b"48 65 6C6c 6F>"), b"Hello");
        assert_eq!(ascii85(b"87cURD]i,\"Ebo80~>"), b"Hello World!");
        assert_eq!(run_length(&[2, b'a', b'b', b'c', 254, b'z', 128], 100), b"abczzz");
    }

    #[test]
    fn lzw_pdf_example() {
        // the example of the PDF specification: 45 45 45 45 45 65 45 45 45 66
        let enc = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(lzw(&enc, true, 100), [45, 45, 45, 45, 45, 65, 45, 45, 45, 66]);
    }

    #[test]
    fn png_up_predictor() {
        let d = vec![2, 1, 2, 3, 2, 1, 1, 1];
        assert_eq!(predictor(d, 12, 1, 8, 3), [1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn garbage_never_panics() {
        let mut x = 12345u32;
        for n in 0..300 {
            let g: Vec<u8> = (0..n * 3).map(|_| { x = x.wrapping_mul(1103515245).wrapping_add(12345); (x >> 16) as u8 }).collect();
            let _ = inflate(&g, 10_000);
            let _ = lzw(&g, true, 10_000);
            let _ = ascii85(&g);
            let _ = run_length(&g, 10_000);
            let _ = predictor(g.clone(), 15, 3, 8, 7);
            let _ = predictor(g, 2, 3, 8, 7);
        }
    }
}
