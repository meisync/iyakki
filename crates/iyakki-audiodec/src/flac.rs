//! FLAC, written in-house from the public format description (no library, no table copied from one). A stream decoder: bytes in,
//! interleaved 16-bit samples out. Handles the whole format that FLAC files use: constant, verbatim, fixed and LPC subframes, both Rice
//! codings, wasted bits, the three stereo decorrelations, block sizes and sample rates from the frame header, 8 to 24 bits per sample
//! (shifted to 16), one or two channels (the helper's limit). Every count read from the stream is checked against the stream's own
//! limits before it is used; a damaged frame (its CRC-16 does not match, or a field is out of range) is skipped by looking for the next
//! frame sync code, and nothing panics.

const MAX_BLOCK: usize = 65_535;

#[derive(Debug, PartialEq)]
enum E {
    /// The frame is not complete yet.
    More,
    /// The bytes here are not a frame.
    Bad,
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn bit(&mut self) -> Result<u32, E> {
        let byte = self.pos >> 3;
        if byte >= self.d.len() {
            return Err(E::More);
        }
        let b = (self.d[byte] >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Ok(u32::from(b))
    }
    fn read(&mut self, n: u32) -> Result<u32, E> {
        let mut v: u64 = 0;
        for _ in 0..n {
            v = (v << 1) | u64::from(self.bit()?);
        }
        Ok(v as u32)
    }
    fn signed(&mut self, n: u32) -> Result<i32, E> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.read(n)?;
        Ok(if n >= 32 { v as i32 } else { ((v << (32 - n)) as i32) >> (32 - n) })
    }
    fn unary(&mut self) -> Result<u32, E> {
        let mut n = 0;
        while self.bit()? == 0 {
            n += 1;
            if n > 1 << 20 {
                return Err(E::Bad);
            }
        }
        Ok(n)
    }
    fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }
}

pub fn crc8(d: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in d {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}

pub fn crc16(d: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in d {
        c ^= u16::from(b) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
        }
    }
    c
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StreamInfo {
    pub rate: u32,
    pub channels: usize,
    pub bits: u32,
    /// 0 when the stream does not say.
    pub total: u64,
}

pub struct Decoder {
    buf: Vec<u8>,
    info: Option<StreamInfo>,
    marker_done: bool,
    meta_done: bool,
    pub finished: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::new()
    }
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder { buf: Vec::new(), info: None, marker_done: false, meta_done: false, finished: false }
    }

    pub fn info(&self) -> Option<StreamInfo> {
        self.info.filter(|_| self.meta_done)
    }

    /// Feeds bytes; returns the samples of every frame that is complete now.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<i16>, String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        if !self.marker_done {
            if self.buf.len() < 4 {
                return Ok(out);
            }
            if &self.buf[..4] != b"fLaC" {
                return Err("this is not a FLAC stream".into());
            }
            self.buf.drain(..4);
            self.marker_done = true;
        }
        while !self.meta_done {
            if self.buf.len() < 4 {
                return Ok(out);
            }
            let last = self.buf[0] & 0x80 != 0;
            let kind = self.buf[0] & 0x7f;
            let len = (usize::from(self.buf[1]) << 16) | (usize::from(self.buf[2]) << 8) | usize::from(self.buf[3]);
            if len > 16 << 20 {
                return Err("a FLAC metadata block is too large".into());
            }
            if self.buf.len() < 4 + len {
                return Ok(out);
            }
            if kind == 0 {
                if len < 34 {
                    return Err("the FLAC stream info is too short".into());
                }
                let b = &self.buf[4..4 + 34];
                let v = u64::from_be_bytes([b[10], b[11], b[12], b[13], b[14], b[15], b[16], b[17]]);
                let rate = (v >> 44) as u32;
                let channels = (((v >> 41) & 7) + 1) as usize;
                let bits = (((v >> 36) & 31) + 1) as u32;
                let total = v & 0xF_FFFF_FFFF;
                if !(1..=2).contains(&channels) {
                    return Err(format!("this FLAC stream has {channels} channels; 1 and 2 are played"));
                }
                if !(8..=24).contains(&bits) {
                    return Err(format!("this FLAC stream has {bits} bits per sample; 8 to 24 are played"));
                }
                if !(1..=655_350).contains(&rate) {
                    return Err("this FLAC stream has no sample rate".into());
                }
                self.info = Some(StreamInfo { rate, channels, bits, total });
            }
            self.buf.drain(..4 + len);
            if last {
                if self.info.is_none() {
                    return Err("this FLAC stream has no stream info".into());
                }
                self.meta_done = true;
            }
        }
        let info = self.info.unwrap_or_default();
        loop {
            // find a frame sync code
            let Some(at) = (0..self.buf.len().saturating_sub(1)).find(|&i| self.buf[i] == 0xFF && self.buf[i + 1] & 0xFE == 0xF8) else {
                let keep = self.buf.len().min(1);
                let cut = self.buf.len() - keep;
                self.buf.drain(..cut);
                break;
            };
            self.buf.drain(..at);
            match decode_frame(&self.buf, &info) {
                Ok((used, pcm)) => {
                    self.buf.drain(..used);
                    out.extend(pcm);
                }
                Err(E::More) => {
                    if self.finished {
                        self.buf.clear();
                    }
                    break;
                }
                Err(E::Bad) => {
                    // not a frame after all: look further on
                    self.buf.drain(..1);
                }
            }
            if self.buf.len() > 8 << 20 {
                self.buf.clear();
            }
        }
        Ok(out)
    }
}

fn utf8_number(b: &mut Bits) -> Result<u64, E> {
    let first = b.read(8)?;
    let extra = if first & 0x80 == 0 {
        return Ok(u64::from(first));
    } else if first & 0xE0 == 0xC0 {
        1
    } else if first & 0xF0 == 0xE0 {
        2
    } else if first & 0xF8 == 0xF0 {
        3
    } else if first & 0xFC == 0xF8 {
        4
    } else if first & 0xFE == 0xFC {
        5
    } else if first == 0xFE {
        6
    } else {
        return Err(E::Bad);
    };
    let mut v = u64::from(first & (0x3F >> extra));
    for _ in 0..extra {
        let c = b.read(8)?;
        if c & 0xC0 != 0x80 {
            return Err(E::Bad);
        }
        v = (v << 6) | u64::from(c & 0x3F);
    }
    Ok(v)
}

/// One frame from the start of `d`: the bytes it used and its samples.
fn decode_frame(d: &[u8], info: &StreamInfo) -> Result<(usize, Vec<i16>), E> {
    let mut b = Bits { d, pos: 0 };
    if b.read(14)? != 0x3FFE {
        return Err(E::Bad);
    }
    if b.read(1)? != 0 {
        return Err(E::Bad);
    }
    let _blocking = b.read(1)?;
    let bs_code = b.read(4)?;
    let rate_code = b.read(4)?;
    let chan_code = b.read(4)?;
    let size_code = b.read(3)?;
    if b.read(1)? != 0 {
        return Err(E::Bad);
    }
    utf8_number(&mut b)?;
    let block = match bs_code {
        0 => return Err(E::Bad),
        1 => 192,
        2..=5 => 576usize << (bs_code - 2),
        6 => b.read(8)? as usize + 1,
        7 => b.read(16)? as usize + 1,
        _ => 256usize << (bs_code - 8),
    };
    match rate_code {
        12 => {
            b.read(8)?;
        }
        13 | 14 => {
            b.read(16)?;
        }
        15 => return Err(E::Bad),
        _ => {}
    }
    let header_len = b.pos / 8;
    let crc = b.read(8)? as u8;
    if crc8(&d[..header_len]) != crc {
        return Err(E::Bad);
    }
    if block > MAX_BLOCK {
        return Err(E::Bad);
    }
    let bps = match size_code {
        0 => info.bits,
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        _ => return Err(E::Bad),
    };
    let (channels, mode) = match chan_code {
        0 => (1, 0),
        1 => (2, 0),
        8 => (2, 1),
        9 => (2, 2),
        10 => (2, 3),
        _ => return Err(E::Bad),
    };
    if channels != info.channels {
        return Err(E::Bad);
    }
    let mut chans: Vec<Vec<i32>> = Vec::with_capacity(channels);
    for c in 0..channels {
        // the side channel carries one more bit
        let extra = match (mode, c) {
            (1, 1) | (2, 0) | (3, 1) => 1,
            _ => 0,
        };
        chans.push(subframe(&mut b, block, bps + extra)?);
    }
    b.align();
    let end = b.pos / 8;
    let crc16_at = end;
    if d.len() < crc16_at + 2 {
        return Err(E::More);
    }
    let want = u16::from_be_bytes([d[crc16_at], d[crc16_at + 1]]);
    if crc16(&d[..crc16_at]) != want {
        return Err(E::Bad);
    }
    match mode {
        1 => {
            for i in 0..block {
                chans[1][i] = chans[0][i].wrapping_sub(chans[1][i]);
            }
        }
        2 => {
            for i in 0..block {
                chans[0][i] = chans[0][i].wrapping_add(chans[1][i]);
            }
        }
        3 => {
            for i in 0..block {
                let side = chans[1][i];
                let mid = (chans[0][i] << 1) | (side & 1);
                chans[0][i] = (mid.wrapping_add(side)) >> 1;
                chans[1][i] = (mid.wrapping_sub(side)) >> 1;
            }
        }
        _ => {}
    }
    let mut pcm = Vec::with_capacity(block * channels);
    for i in 0..block {
        for c in chans.iter() {
            let s = c[i];
            let v = if bps > 16 { s >> (bps - 16) } else { s << (16 - bps) };
            pcm.push(v.clamp(-32768, 32767) as i16);
        }
    }
    Ok((crc16_at + 2, pcm))
}

fn subframe(b: &mut Bits, block: usize, bps: u32) -> Result<Vec<i32>, E> {
    if b.read(1)? != 0 {
        return Err(E::Bad);
    }
    let kind = b.read(6)?;
    let mut wasted = 0;
    if b.read(1)? == 1 {
        wasted = b.unary()? + 1;
        if wasted >= bps {
            return Err(E::Bad);
        }
    }
    let bps = bps - wasted;
    let mut x = vec![0i32; block];
    match kind {
        0 => {
            let v = b.signed(bps)?;
            x.iter_mut().for_each(|s| *s = v);
        }
        1 => {
            for s in x.iter_mut() {
                *s = b.signed(bps)?;
            }
        }
        8..=12 => {
            let order = (kind - 8) as usize;
            if order > block {
                return Err(E::Bad);
            }
            for s in x.iter_mut().take(order) {
                *s = b.signed(bps)?;
            }
            residual(b, block, order, &mut x)?;
            for i in order..block {
                let p: i64 = match order {
                    0 => 0,
                    1 => i64::from(x[i - 1]),
                    2 => 2 * i64::from(x[i - 1]) - i64::from(x[i - 2]),
                    3 => 3 * i64::from(x[i - 1]) - 3 * i64::from(x[i - 2]) + i64::from(x[i - 3]),
                    _ => 4 * i64::from(x[i - 1]) - 6 * i64::from(x[i - 2]) + 4 * i64::from(x[i - 3]) - i64::from(x[i - 4]),
                };
                x[i] = x[i].wrapping_add(p as i32);
            }
        }
        32..=63 => {
            let order = (kind - 31) as usize;
            if order > block {
                return Err(E::Bad);
            }
            for s in x.iter_mut().take(order) {
                *s = b.signed(bps)?;
            }
            let prec = b.read(4)? + 1;
            if prec == 16 {
                return Err(E::Bad);
            }
            let shift = b.signed(5)?;
            if shift < 0 {
                return Err(E::Bad);
            }
            let mut coef = Vec::with_capacity(order);
            for _ in 0..order {
                coef.push(i64::from(b.signed(prec)?));
            }
            residual(b, block, order, &mut x)?;
            for i in order..block {
                let mut p: i64 = 0;
                for (j, c) in coef.iter().enumerate() {
                    p += c * i64::from(x[i - 1 - j]);
                }
                x[i] = x[i].wrapping_add((p >> shift) as i32);
            }
        }
        _ => return Err(E::Bad),
    }
    if wasted > 0 {
        for s in x.iter_mut() {
            *s <<= wasted;
        }
    }
    Ok(x)
}

fn residual(b: &mut Bits, block: usize, order: usize, x: &mut [i32]) -> Result<(), E> {
    let method = b.read(2)?;
    if method > 1 {
        return Err(E::Bad);
    }
    let (pbits, escape) = if method == 0 { (4, 15) } else { (5, 31) };
    let porder = b.read(4)?;
    let parts = 1usize << porder;
    if block % parts != 0 || (block >> porder) < order {
        return Err(E::Bad);
    }
    let mut i = order;
    for p in 0..parts {
        let n = (block >> porder) - if p == 0 { order } else { 0 };
        let param = b.read(pbits)?;
        if param == escape {
            let bits = b.read(5)?;
            for _ in 0..n {
                x[i] = b.signed(bits)?;
                i += 1;
            }
        } else {
            for _ in 0..n {
                let q = b.unary()?;
                let r = if param > 0 { b.read(param)? } else { 0 };
                let u = (q << param) | r;
                x[i] = ((u >> 1) as i32) ^ -((u & 1) as i32);
                i += 1;
            }
        }
    }
    Ok(())
}
