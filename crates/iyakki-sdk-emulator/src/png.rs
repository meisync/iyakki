//! A small PNG writer and reader for the pictures the emulator compares (8-bit RGBA, no interlace).
//!
//! The writer packs with fixed-Huffman deflate and "same as the pixel to the left" matches, so a flat tile picture is small. The reader
//! understands stored and fixed-Huffman blocks, which is everything this writer produces; a PNG saved by another program (dynamic Huffman)
//! is refused with a message telling the developer to run `iyakki sdk test --update`.

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, n: u32) {
        self.acc |= (v as u64) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    /// A Huffman code goes out most significant bit first.
    fn code(&mut self, c: u32, n: u32) {
        let mut r = 0;
        for i in 0..n {
            r |= ((c >> i) & 1) << (n - 1 - i);
        }
        self.put(r, n);
    }
    fn lit(&mut self, v: u32) {
        match v {
            0..=143 => self.code(0x30 + v, 8),
            144..=255 => self.code(0x190 + v - 144, 9),
            256..=279 => self.code(v - 256, 7),
            _ => self.code(0xc0 + v - 280, 8),
        }
    }
    fn flush(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
            self.acc = 0;
            self.n = 0;
        }
    }
}

const LEN_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

fn deflate_fixed(data: &[u8]) -> Vec<u8> {
    let mut b = Bits { out: Vec::new(), acc: 0, n: 0 };
    b.put(1, 1); // final block
    b.put(1, 2); // fixed Huffman
    let mut i = 0;
    while i < data.len() {
        // a run that repeats the pixel four bytes back (distance 4)
        let mut run = 0;
        if i >= 4 {
            while run < 258 && i + run < data.len() && data[i + run] == data[i + run - 4] {
                run += 1;
            }
        }
        if run >= 3 {
            let k = LEN_BASE.iter().rposition(|&base| base as usize <= run).unwrap_or(0);
            b.lit(257 + k as u32);
            b.put((run - LEN_BASE[k] as usize) as u32, LEN_EXTRA[k] as u32);
            b.code(3, 5); // distance code 3 = distance 4
            i += run;
        } else {
            b.lit(data[i] as u32);
            i += 1;
        }
    }
    b.lit(256);
    b.flush();
    b.out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut c = kind.to_vec();
    c.extend_from_slice(body);
    out.extend_from_slice(&c);
    out.extend_from_slice(&crc32(&c).to_be_bytes());
}

/// Encode an RGBA picture (`w * h * 4` bytes) as a PNG.
pub fn encode(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks((w as usize * 4).max(1)).take(h as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    z.extend(deflate_fixed(&raw));
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
    bit: u32,
}

impl Reader<'_> {
    fn bit(&mut self) -> Result<u32, String> {
        let byte = *self.d.get(self.pos).ok_or("the picture data ends early")?;
        let v = (byte >> self.bit) as u32 & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.pos += 1;
        }
        Ok(v)
    }
    fn bits(&mut self, n: u32) -> Result<u32, String> {
        let mut v = 0;
        for i in 0..n {
            v |= self.bit()? << i;
        }
        Ok(v)
    }
    fn code(&mut self, n: u32) -> Result<u32, String> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }
}

fn inflate(d: &[u8]) -> Result<Vec<u8>, String> {
    let mut r = Reader { d, pos: 0, bit: 0 };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = r.bit()?;
        match r.bits(2)? {
            0 => {
                if r.bit != 0 {
                    r.bit = 0;
                    r.pos += 1;
                }
                let h = d.get(r.pos..r.pos + 4).ok_or("the picture data ends early")?;
                let n = u16::from_le_bytes([h[0], h[1]]) as usize;
                r.pos += 4;
                out.extend_from_slice(d.get(r.pos..r.pos + n).ok_or("the picture data ends early")?);
                r.pos += n;
            }
            1 => loop {
                let mut c = r.code(7)?;
                let sym = if c <= 23 {
                    256 + c
                } else {
                    c = (c << 1) | r.bit()?;
                    if (48..=191).contains(&c) {
                        c - 48
                    } else if (192..=199).contains(&c) {
                        280 + c - 192
                    } else {
                        c = (c << 1) | r.bit()?;
                        144 + c.checked_sub(400).ok_or("bad picture data")?
                    }
                };
                match sym {
                    0..=255 => out.push(sym as u8),
                    256 => break,
                    257..=285 => {
                        let k = (sym - 257) as usize;
                        let len = LEN_BASE[k] as usize + r.bits(LEN_EXTRA[k] as u32)? as usize;
                        let dc = r.code(5)? as usize;
                        let dist = *DIST_BASE.get(dc).ok_or("bad picture data")? as usize + r.bits(DIST_EXTRA[dc] as u32)? as usize;
                        if dist > out.len() {
                            return Err("bad picture data".into());
                        }
                        for _ in 0..len {
                            out.push(out[out.len() - dist]);
                        }
                    }
                    _ => return Err("bad picture data".into()),
                }
            },
            _ => return Err("this PNG was not written by the emulator (it uses a packing the emulator does not read): run `iyakki sdk test --update` to write it again".into()),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

/// Decode a PNG written by [`encode`]: returns `(w, h, rgba)`.
pub fn decode(png: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if png.len() < 8 || png[..8] != [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] {
        return Err("not a PNG file".into());
    }
    let (mut pos, mut w, mut h, mut idat) = (8usize, 0u32, 0u32, Vec::new());
    while pos + 8 <= png.len() {
        let n = u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]]) as usize;
        let kind = &png[pos + 4..pos + 8];
        let body = png.get(pos + 8..pos + 8 + n).ok_or("the PNG ends early")?;
        match kind {
            b"IHDR" if n == 13 => {
                w = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                h = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
                if body[8..] != [8, 6, 0, 0, 0] {
                    return Err("only 8-bit RGBA PNG pictures are read (the emulator writes that)".into());
                }
            }
            b"IDAT" => idat.extend_from_slice(body),
            _ => {}
        }
        pos += 12 + n;
    }
    if w == 0 || h == 0 || w > 16384 || h > 16384 || idat.len() < 6 {
        return Err("the PNG has no picture".into());
    }
    let raw = inflate(&idat[2..])?;
    let stride = w as usize * 4;
    if raw.len() != (stride + 1) * h as usize {
        return Err("the PNG picture has the wrong size".into());
    }
    let mut rgba = Vec::with_capacity(stride * h as usize);
    for row in raw.chunks(stride + 1) {
        if row[0] != 0 {
            return Err("this PNG was not written by the emulator (it uses row filters): run `iyakki sdk test --update` to write it again".into());
        }
        rgba.extend_from_slice(&row[1..]);
    }
    Ok((w, h, rgba))
}
