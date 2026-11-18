//! A small DEFLATE decoder (RFC 1951) for ZRLE: one zlib stream runs through a whole connection, each rectangle's data ends at a
//! block boundary (the server flushes after each), and back-references may reach into earlier rectangles, so the last 32 KiB of
//! output are kept between calls. The output of one call is capped by the caller; malformed data is an error, never a panic.

const WINDOW: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflateError {
    Truncated,
    BadHeader,
    BadBlock,
    BadCode,
    BadDistance,
    TooBig,
}

pub struct Inflater {
    hist: Vec<u8>,
    header_done: bool,
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    bit: u32,
}

impl<'a> Bits<'a> {
    fn one(&mut self) -> Result<u32, InflateError> {
        let b = *self.d.get(self.pos).ok_or(InflateError::Truncated)?;
        let v = (b >> self.bit) as u32 & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.pos += 1;
        }
        Ok(v)
    }
    fn n(&mut self, n: u32) -> Result<u32, InflateError> {
        let mut v = 0;
        for i in 0..n {
            v |= self.one()? << i;
        }
        Ok(v)
    }
    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.pos += 1;
        }
    }
    fn at_end(&self) -> bool {
        self.pos >= self.d.len()
    }
}

struct Huff {
    count: [u16; 16],
    symbol: Vec<u16>,
}

impl Huff {
    fn new(lengths: &[u8]) -> Result<Huff, InflateError> {
        let mut count = [0u16; 16];
        for &l in lengths {
            count[l as usize & 15] += 1;
        }
        let mut left: i32 = 1;
        for len in 1..16 {
            left <<= 1;
            left -= count[len] as i32;
            if left < 0 {
                return Err(InflateError::BadCode);
            }
        }
        let mut offs = [0u16; 16];
        for len in 1..15 {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Huff { count, symbol })
    }

    fn decode(&self, b: &mut Bits) -> Result<u16, InflateError> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.one()? as i32;
            let c = self.count[len] as i32;
            if code - c < first {
                return self.symbol.get((index + (code - first)) as usize).copied().ok_or(InflateError::BadCode);
            }
            index += c;
            first += c;
            first <<= 1;
            code <<= 1;
        }
        Err(InflateError::BadCode)
    }
}

const LBASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEXT: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DBASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DEXT: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

impl Default for Inflater {
    fn default() -> Self {
        Self::new()
    }
}

impl Inflater {
    pub fn new() -> Inflater {
        Inflater { hist: Vec::new(), header_done: false }
    }

    /// Decode one chunk (a whole number of blocks) and return its output, at most `max_out` bytes.
    pub fn inflate(&mut self, input: &[u8], max_out: usize) -> Result<Vec<u8>, InflateError> {
        let mut b = Bits { d: input, pos: 0, bit: 0 };
        if !self.header_done {
            let (c, f) = (*input.first().ok_or(InflateError::Truncated)?, *input.get(1).ok_or(InflateError::Truncated)?);
            if c & 0x0f != 8 || (c >> 4) > 7 || ((c as u32) << 8 | f as u32) % 31 != 0 || f & 0x20 != 0 {
                return Err(InflateError::BadHeader);
            }
            b.pos = 2;
            self.header_done = true;
        }
        let base = self.hist.len();
        let mut out = std::mem::take(&mut self.hist);
        let r = (|| -> Result<(), InflateError> {
            while !b.at_end() {
                let last = b.one()? == 1;
                match b.n(2)? {
                    0 => {
                        b.align();
                        let (l, nl) = (b.n(16)?, b.n(16)?);
                        if l != !nl & 0xffff {
                            return Err(InflateError::BadBlock);
                        }
                        let l = l as usize;
                        let s = input.get(b.pos..b.pos + l).ok_or(InflateError::Truncated)?;
                        if out.len() - base + l > max_out {
                            return Err(InflateError::TooBig);
                        }
                        out.extend_from_slice(s);
                        b.pos += l;
                    }
                    1 => {
                        let mut l = [8u8; 288];
                        l[144..256].fill(9);
                        l[256..280].fill(7);
                        let lit = Huff::new(&l)?;
                        let dist = Huff::new(&[5u8; 30])?;
                        codes(&mut b, &mut out, base, max_out, &lit, &dist)?;
                    }
                    2 => {
                        let nlen = b.n(5)? as usize + 257;
                        let ndist = b.n(5)? as usize + 1;
                        let ncode = b.n(4)? as usize + 4;
                        if nlen > 286 || ndist > 30 {
                            return Err(InflateError::BadBlock);
                        }
                        const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                        let mut lens = [0u8; 320];
                        for &o in ORDER.iter().take(ncode) {
                            lens[o] = b.n(3)? as u8;
                        }
                        let ch = Huff::new(&lens[..19])?;
                        let mut lens = [0u8; 320];
                        let mut i = 0;
                        while i < nlen + ndist {
                            let sym = ch.decode(&mut b)?;
                            if sym < 16 {
                                lens[i] = sym as u8;
                                i += 1;
                            } else {
                                let (prev, rep) = match sym {
                                    16 => {
                                        if i == 0 {
                                            return Err(InflateError::BadCode);
                                        }
                                        (lens[i - 1], 3 + b.n(2)? as usize)
                                    }
                                    17 => (0, 3 + b.n(3)? as usize),
                                    _ => (0, 11 + b.n(7)? as usize),
                                };
                                if i + rep > nlen + ndist {
                                    return Err(InflateError::BadCode);
                                }
                                lens[i..i + rep].fill(prev);
                                i += rep;
                            }
                        }
                        if lens[256] == 0 {
                            return Err(InflateError::BadCode);
                        }
                        let lit = Huff::new(&lens[..nlen])?;
                        let dist = Huff::new(&lens[nlen..nlen + ndist])?;
                        codes(&mut b, &mut out, base, max_out, &lit, &dist)?;
                    }
                    _ => return Err(InflateError::BadBlock),
                }
                if last {
                    b.align();
                    break;
                }
            }
            Ok(())
        })();
        match r {
            Ok(()) => {
                let produced = out[base..].to_vec();
                let keep = out.len().saturating_sub(WINDOW);
                out.drain(..keep);
                self.hist = out;
                Ok(produced)
            }
            Err(e) => {
                // the stream is unusable after an error: the caller drops the connection
                out.truncate(base);
                self.hist = out;
                Err(e)
            }
        }
    }
}

fn codes(b: &mut Bits, out: &mut Vec<u8>, base: usize, max_out: usize, lit: &Huff, dist: &Huff) -> Result<(), InflateError> {
    loop {
        let s = lit.decode(b)? as usize;
        if s < 256 {
            if out.len() - base >= max_out {
                return Err(InflateError::TooBig);
            }
            out.push(s as u8);
        } else if s == 256 {
            return Ok(());
        } else {
            let s = s - 257;
            if s >= 29 {
                return Err(InflateError::BadCode);
            }
            let len = LBASE[s] as usize + b.n(LEXT[s] as u32)? as usize;
            let d = dist.decode(b)? as usize;
            if d >= 30 {
                return Err(InflateError::BadCode);
            }
            let dd = DBASE[d] as usize + b.n(DEXT[d] as u32)? as usize;
            if dd > out.len() {
                return Err(InflateError::BadDistance);
            }
            if out.len() - base + len > max_out {
                return Err(InflateError::TooBig);
            }
            for _ in 0..len {
                let v = out[out.len() - dd];
                out.push(v);
            }
        }
    }
}
