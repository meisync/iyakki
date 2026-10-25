//! An in-house `.xz` reader (LZMA2, with the branch filters x86, ARM, ARM Thumb, ARM64, PowerPC, SPARC and delta before it, as 7-Zip's
//! own Linux builds use x86 BCJ; IA-64 and RISC-V are refused) and an MD5 digest, so IYAKKI can unpack the static
//! ffmpeg build the vendor ships as `.tar.xz` and check it against the `.md5` file next to it, with no extra program or library.
//!
//! The reader decodes one or more streams of blocks into any [`Write`]; the check bytes (CRC) are skipped (the caller checks the
//! whole file against the vendor's digest). A dictionary larger than [`MAX_DICT`] is refused.

use std::io::Write;

/// The largest dictionary accepted (the vendor's builds use 8 MB).
pub const MAX_DICT: usize = 256 << 20;

const MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0];

struct Rc<'a> {
    d: &'a [u8],
    i: usize,
    range: u32,
    code: u32,
}

impl<'a> Rc<'a> {
    fn new(d: &'a [u8]) -> Result<Rc<'a>, String> {
        if d.len() < 5 || d[0] != 0 {
            return Err("damaged LZMA data".into());
        }
        Ok(Rc { d, i: 5, range: 0xFFFF_FFFF, code: u32::from_be_bytes([d[1], d[2], d[3], d[4]]) })
    }
    fn norm(&mut self) {
        if self.range < (1 << 24) {
            self.range <<= 8;
            let b = self.d.get(self.i).copied().unwrap_or(0);
            self.i += 1;
            self.code = (self.code << 8) | b as u32;
        }
    }
    fn bit(&mut self, p: &mut u16) -> u32 {
        self.norm();
        let bound = (self.range >> 11) * (*p as u32);
        if self.code < bound {
            self.range = bound;
            *p += (2048 - *p) >> 5;
            0
        } else {
            self.range -= bound;
            self.code -= bound;
            *p -= *p >> 5;
            1
        }
    }
    fn direct(&mut self, n: u32) -> u32 {
        let mut r = 0;
        for _ in 0..n {
            self.norm();
            self.range >>= 1;
            let b = if self.code >= self.range { self.code -= self.range; 1 } else { 0 };
            r = (r << 1) | b;
        }
        r
    }
    fn tree(&mut self, probs: &mut [u16], n: u32) -> u32 {
        let mut m = 1usize;
        for _ in 0..n {
            m = (m << 1) | self.bit(&mut probs[m]) as usize;
        }
        (m - (1 << n)) as u32
    }
    fn rtree(&mut self, probs: &mut [u16], base: usize, n: u32) -> u32 {
        let (mut m, mut r) = (1usize, 0u32);
        for i in 0..n {
            let b = self.bit(&mut probs[base + m - 1]);
            m = (m << 1) | b as usize;
            r |= b << i;
        }
        r
    }
}

struct Len {
    choice: u16,
    choice2: u16,
    low: [[u16; 8]; 16],
    mid: [[u16; 8]; 16],
    high: [u16; 256],
}

impl Len {
    fn new() -> Len {
        Len { choice: 1024, choice2: 1024, low: [[1024; 8]; 16], mid: [[1024; 8]; 16], high: [1024; 256] }
    }
    fn decode(&mut self, rc: &mut Rc, ps: usize) -> u32 {
        if rc.bit(&mut self.choice) == 0 {
            rc.tree(&mut self.low[ps], 3)
        } else if rc.bit(&mut self.choice2) == 0 {
            8 + rc.tree(&mut self.mid[ps], 3)
        } else {
            16 + rc.tree(&mut self.high, 8)
        }
    }
}

struct Model {
    lc: u32,
    lp: u32,
    pb: u32,
    state: usize,
    reps: [u32; 4],
    is_match: [u16; 12 << 4],
    is_rep: [u16; 12],
    g0: [u16; 12],
    g1: [u16; 12],
    g2: [u16; 12],
    rep0_long: [u16; 12 << 4],
    slot: [[u16; 64]; 4],
    special: [u16; 114],
    align: [u16; 16],
    len: Len,
    rep_len: Len,
    lit: Vec<u16>,
}

impl Model {
    fn new() -> Model {
        Model { lc: 0, lp: 0, pb: 0, state: 0, reps: [0; 4], is_match: [1024; 192], is_rep: [1024; 12], g0: [1024; 12], g1: [1024; 12], g2: [1024; 12], rep0_long: [1024; 192], slot: [[1024; 64]; 4], special: [1024; 114], align: [1024; 16], len: Len::new(), rep_len: Len::new(), lit: Vec::new() }
    }
    fn reset(&mut self) {
        let (lc, lp, pb) = (self.lc, self.lp, self.pb);
        *self = Model::new();
        self.lc = lc;
        self.lp = lp;
        self.pb = pb;
        self.lit = vec![1024; 0x300 << (lc + lp)];
    }
}

struct Dict<'w, W: Write> {
    buf: Vec<u8>,
    pos: usize,
    full: usize,
    /// Bytes since the last dictionary reset (LZMA position states count from there).
    total: u64,
    out: &'w mut W,
    pending: Vec<u8>,
    written: u64,
    limit: u64,
}

impl<'w, W: Write> Dict<'w, W> {
    fn put(&mut self, b: u8) -> Result<(), String> {
        self.buf[self.pos] = b;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        if self.full < self.buf.len() {
            self.full += 1;
        }
        self.total += 1;
        self.pending.push(b);
        if self.pending.len() >= 1 << 20 {
            self.flush()?;
        }
        Ok(())
    }
    fn back(&self, dist: usize) -> u8 {
        let n = self.buf.len();
        self.buf[(self.pos + n - dist) % n]
    }
    fn flush(&mut self) -> Result<(), String> {
        self.written += self.pending.len() as u64;
        if self.written > self.limit {
            return Err("the archive unpacks to more than allowed".into());
        }
        self.out.write_all(&self.pending).map_err(|e| e.to_string())?;
        self.pending.clear();
        Ok(())
    }
}

fn lzma_chunk<W: Write>(m: &mut Model, d: &mut Dict<W>, data: &[u8], unpacked: usize) -> Result<(), String> {
    let mut rc = Rc::new(data)?;
    let end = d.total + unpacked as u64;
    let bad = || "damaged LZMA data".to_string();
    while d.total < end {
        let ps = (d.total as usize) & ((1 << m.pb) - 1);
        let st = m.state;
        if rc.bit(&mut m.is_match[(st << 4) | ps]) == 0 {
            let prev = if d.full > 0 { d.back(1) as usize } else { 0 };
            let ls = (((d.total as usize) & ((1 << m.lp) - 1)) << m.lc) + (prev >> (8 - m.lc));
            let probs = &mut m.lit[0x300 * ls..0x300 * (ls + 1)];
            let mut sym = 1usize;
            if st >= 7 {
                let mut mb = d.back(m.reps[0] as usize + 1) as usize;
                loop {
                    let mbit = (mb >> 7) & 1;
                    mb <<= 1;
                    let b = rc.bit(&mut probs[((1 + mbit) << 8) + sym]) as usize;
                    sym = (sym << 1) | b;
                    if mbit != b || sym >= 0x100 {
                        break;
                    }
                }
            }
            while sym < 0x100 {
                sym = (sym << 1) | rc.bit(&mut probs[sym]) as usize;
            }
            d.put(sym as u8)?;
            m.state = if st < 4 { 0 } else if st < 10 { st - 3 } else { st - 6 };
            continue;
        }
        let len;
        if rc.bit(&mut m.is_rep[st]) == 0 {
            m.reps = [0, m.reps[0], m.reps[1], m.reps[2]];
            len = m.len.decode(&mut rc, ps);
            m.state = if st < 7 { 7 } else { 10 };
            let ls = (len as usize).min(3);
            let slot = rc.tree(&mut m.slot[ls], 6);
            let dist = if slot < 4 {
                slot
            } else {
                let nd = (slot >> 1) - 1;
                let base = (2 | (slot & 1)) << nd;
                if slot < 14 {
                    base + rc.rtree(&mut m.special, (base - slot) as usize, nd)
                } else {
                    base.wrapping_add(rc.direct(nd - 4) << 4).wrapping_add(rc.rtree(&mut m.align, 1, 4))
                }
            };
            if dist == 0xFFFF_FFFF {
                return Err(bad());
            }
            m.reps[0] = dist;
        } else {
            if d.full == 0 {
                return Err(bad());
            }
            if rc.bit(&mut m.g0[st]) == 0 {
                if rc.bit(&mut m.rep0_long[(st << 4) | ps]) == 0 {
                    m.state = if st < 7 { 9 } else { 11 };
                    if m.reps[0] as usize >= d.full {
                        return Err(bad());
                    }
                    let b = d.back(m.reps[0] as usize + 1);
                    d.put(b)?;
                    continue;
                }
            } else {
                let dist;
                if rc.bit(&mut m.g1[st]) == 0 {
                    dist = m.reps[1];
                } else {
                    if rc.bit(&mut m.g2[st]) == 0 {
                        dist = m.reps[2];
                    } else {
                        dist = m.reps[3];
                        m.reps[3] = m.reps[2];
                    }
                    m.reps[2] = m.reps[1];
                }
                m.reps[1] = m.reps[0];
                m.reps[0] = dist;
            }
            len = m.rep_len.decode(&mut rc, ps);
            m.state = if st < 7 { 8 } else { 11 };
        }
        let dist = m.reps[0] as usize + 1;
        if dist > d.full {
            return Err(bad());
        }
        for _ in 0..len + 2 {
            let b = d.back(dist);
            d.put(b)?;
        }
    }
    if d.total != end {
        return Err(bad());
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ the simple filters

/// A filter that comes before LZMA2 in an xz block header (the branch converters `xz --x86` and friends, and delta). The decoder
/// undoes them on the LZMA2 output. IA64 and RISC-V are not supported (an error, not a guess).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Delta(usize),
    X86,
    Ppc,
    Arm,
    ArmThumb,
    Sparc,
    Arm64,
}

impl Kind {
    fn from_header(id: u64, props: &[u8]) -> Result<Kind, String> {
        let no_props = |k: Kind| if props.is_empty() { Ok(k) } else { Err("an xz filter with options that are not supported (a start offset)".to_string()) };
        match id {
            0x03 if props.len() == 1 => Ok(Kind::Delta(props[0] as usize + 1)),
            0x03 => Err("damaged xz header".into()),
            0x04 => no_props(Kind::X86),
            0x05 => no_props(Kind::Ppc),
            0x07 => no_props(Kind::Arm),
            0x08 => no_props(Kind::ArmThumb),
            0x09 => no_props(Kind::Sparc),
            0x0A => no_props(Kind::Arm64),
            0x06 => Err("an xz file with the IA-64 filter is not supported".into()),
            0x0B => Err("an xz file with the RISC-V filter is not supported".into()),
            0x21 => Err("damaged xz header".into()),
            _ => Err("an xz file with a filter that is not supported".into()),
        }
    }
}

/// Where decoded bytes go, with a last call that lets filters pass on what they held back.
trait Fin: Write {
    fn fin(&mut self) -> std::io::Result<()>;
}

struct Base<'a>(&'a mut dyn Write);

impl Write for Base<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl Fin for Base<'_> {
    fn fin(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

struct Stage<'a> {
    next: Box<dyn Fin + 'a>,
    kind: Kind,
    buf: Vec<u8>,
    pos: u64,
    // x86 state
    prev_mask: u32,
    prev_pos: u32,
    // delta history
    hist: [u8; 256],
    hpos: usize,
}

fn build_chain<'a>(kinds: &[Kind], out: &'a mut dyn Write) -> Box<dyn Fin + 'a> {
    // the first filter in the header was applied first when compressing, so it is undone last: it is nearest the output
    let mut next: Box<dyn Fin + 'a> = Box::new(Base(out));
    for k in kinds {
        next = Box::new(Stage { next, kind: k.clone(), buf: Vec::new(), pos: 0, prev_mask: 0, prev_pos: (-5i32) as u32, hist: [0; 256], hpos: 0 });
    }
    next
}

fn rd32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn test86(b: u8) -> bool {
    b == 0 || b == 0xFF
}

impl Stage<'_> {
    /// Decode as much of `self.buf` as can be finished; returns how many bytes are final.
    fn process(&mut self) -> usize {
        let now = self.pos as u32;
        let buf = &mut self.buf[..];
        let size = buf.len();
        match self.kind {
            Kind::Delta(dist) => {
                for b in buf.iter_mut() {
                    let v = b.wrapping_add(self.hist[(self.hpos + 256 - dist) & 0xFF]);
                    self.hist[self.hpos & 0xFF] = v;
                    self.hpos = (self.hpos + 1) & 0xFF;
                    *b = v;
                }
                size
            }
            Kind::X86 => {
                const ALLOWED: [bool; 8] = [true, true, true, false, true, false, false, false];
                const BITNO: [u32; 8] = [0, 1, 2, 2, 3, 3, 3, 3];
                if size < 5 {
                    return 0;
                }
                let (mut prev_mask, mut prev_pos) = (self.prev_mask, self.prev_pos);
                if now.wrapping_sub(prev_pos) > 5 {
                    prev_pos = now.wrapping_sub(5);
                }
                let limit = size - 5;
                let mut p = 0usize;
                while p <= limit {
                    let mut b = buf[p];
                    if b != 0xE8 && b != 0xE9 {
                        p += 1;
                        continue;
                    }
                    let offset = now.wrapping_add(p as u32).wrapping_sub(prev_pos);
                    prev_pos = now.wrapping_add(p as u32);
                    if offset > 5 {
                        prev_mask = 0;
                    } else {
                        for _ in 0..offset {
                            prev_mask &= 0x77;
                            prev_mask <<= 1;
                        }
                    }
                    b = buf[p + 4];
                    if test86(b) && ALLOWED[((prev_mask >> 1) & 7) as usize] && (prev_mask >> 1) < 0x10 {
                        let mut src = (b as u32) << 24 | (buf[p + 3] as u32) << 16 | (buf[p + 2] as u32) << 8 | buf[p + 1] as u32;
                        let mut dest;
                        loop {
                            dest = src.wrapping_sub(now.wrapping_add(p as u32).wrapping_add(5));
                            if prev_mask == 0 {
                                break;
                            }
                            let i = BITNO[(prev_mask >> 1) as usize];
                            b = (dest >> (24 - i * 8)) as u8;
                            if !test86(b) {
                                break;
                            }
                            src = dest ^ ((1u32 << (32 - i * 8)) - 1);
                        }
                        buf[p + 4] = !((((dest >> 24) & 1) as u8).wrapping_sub(1));
                        buf[p + 3] = (dest >> 16) as u8;
                        buf[p + 2] = (dest >> 8) as u8;
                        buf[p + 1] = dest as u8;
                        p += 5;
                        prev_mask = 0;
                    } else {
                        p += 1;
                        prev_mask |= 1;
                        if test86(b) {
                            prev_mask |= 0x10;
                        }
                    }
                }
                self.prev_mask = prev_mask;
                self.prev_pos = prev_pos;
                p
            }
            Kind::Arm => {
                let mut i = 0;
                while i + 4 <= size {
                    if buf[i + 3] == 0xEB {
                        let src = ((buf[i + 2] as u32) << 16 | (buf[i + 1] as u32) << 8 | buf[i] as u32) << 2;
                        let dest = src.wrapping_sub(now.wrapping_add(i as u32).wrapping_add(8)) >> 2;
                        buf[i + 2] = (dest >> 16) as u8;
                        buf[i + 1] = (dest >> 8) as u8;
                        buf[i] = dest as u8;
                    }
                    i += 4;
                }
                i
            }
            Kind::ArmThumb => {
                let mut i = 0;
                while i + 4 <= size {
                    if (buf[i + 1] & 0xF8) == 0xF0 && (buf[i + 3] & 0xF8) == 0xF8 {
                        let src = (((buf[i + 1] as u32) & 7) << 19 | (buf[i] as u32) << 11 | ((buf[i + 3] as u32) & 7) << 8 | buf[i + 2] as u32) << 1;
                        let dest = src.wrapping_sub(now.wrapping_add(i as u32).wrapping_add(4)) >> 1;
                        buf[i + 1] = 0xF0 | ((dest >> 19) & 7) as u8;
                        buf[i] = (dest >> 11) as u8;
                        buf[i + 3] = 0xF8 | ((dest >> 8) & 7) as u8;
                        buf[i + 2] = dest as u8;
                        i += 2;
                    }
                    i += 2;
                }
                i
            }
            Kind::Ppc => {
                let mut i = 0;
                while i + 4 <= size {
                    if (buf[i] >> 2) == 0x12 && (buf[i + 3] & 3) == 1 {
                        let src = ((buf[i] as u32) & 3) << 24 | (buf[i + 1] as u32) << 16 | (buf[i + 2] as u32) << 8 | (buf[i + 3] as u32 & !3);
                        let dest = src.wrapping_sub(now.wrapping_add(i as u32));
                        buf[i] = 0x48 | ((dest >> 24) & 3) as u8;
                        buf[i + 1] = (dest >> 16) as u8;
                        buf[i + 2] = (dest >> 8) as u8;
                        buf[i + 3] = (buf[i + 3] & 3) | (dest as u8 & !3);
                    }
                    i += 4;
                }
                i
            }
            Kind::Sparc => {
                let mut i = 0;
                while i + 4 <= size {
                    if (buf[i] == 0x40 && (buf[i + 1] & 0xC0) == 0) || (buf[i] == 0x7F && (buf[i + 1] & 0xC0) == 0xC0) {
                        let src = u32::from_be_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]) << 2;
                        let mut dest = src.wrapping_sub(now.wrapping_add(i as u32)) >> 2;
                        dest = ((0u32.wrapping_sub((dest >> 22) & 1) << 22) & 0x3FFF_FFFF) | (dest & 0x3F_FFFF) | 0x4000_0000;
                        buf[i..i + 4].copy_from_slice(&dest.to_be_bytes());
                    }
                    i += 4;
                }
                i
            }
            Kind::Arm64 => {
                let mut i = 0;
                while i + 4 <= size {
                    let pc = now.wrapping_add(i as u32);
                    let mut instr = rd32le(&buf[i..]);
                    if (instr >> 26) == 0x25 {
                        let src = instr;
                        instr = 0x9400_0000;
                        let pcw = 0u32.wrapping_sub(pc >> 2);
                        instr |= src.wrapping_add(pcw) & 0x03FF_FFFF;
                        buf[i..i + 4].copy_from_slice(&instr.to_le_bytes());
                    } else if (instr & 0x9F00_0000) == 0x9000_0000 {
                        let src = ((instr >> 29) & 3) | ((instr >> 3) & 0x001F_FFFC);
                        if (src.wrapping_add(0x0002_0000) & 0x001C_0000) == 0 {
                            instr &= 0x9000_001F;
                            let pcw = 0u32.wrapping_sub(pc >> 12);
                            let dest = src.wrapping_add(pcw);
                            instr |= (dest & 3) << 29;
                            instr |= (dest & 0x0003_FFFC) << 3;
                            instr |= 0u32.wrapping_sub(dest & 0x0002_0000) & 0x00E0_0000;
                            buf[i..i + 4].copy_from_slice(&instr.to_le_bytes());
                        }
                    }
                    i += 4;
                }
                i
            }
        }
    }
}

impl Write for Stage<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        let n = self.process();
        if n > 0 {
            self.next.write_all(&self.buf[..n])?;
            self.buf.drain(..n);
            self.pos += n as u64;
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Fin for Stage<'_> {
    fn fin(&mut self) -> std::io::Result<()> {
        // what is left (less than one instruction) was never converted by the encoder either
        let rest = std::mem::take(&mut self.buf);
        self.next.write_all(&rest)?;
        self.next.fin()
    }
}

fn varint(b: &[u8], i: &mut usize) -> Result<u64, String> {
    let mut r = 0u64;
    for k in 0..9 {
        let x = *b.get(*i).ok_or("damaged xz header")?;
        *i += 1;
        r |= ((x & 0x7F) as u64) << (7 * k);
        if x & 0x80 == 0 {
            return Ok(r);
        }
    }
    Err("damaged xz header".into())
}

/// Decode the `.xz` file `data` into `out`; at most `limit` bytes are produced.
pub fn decompress<W: Write>(data: &[u8], out: &mut W, limit: u64) -> Result<u64, String> {
    let mut at = 0usize;
    let mut total = 0u64;
    let mut streams = 0;
    while at < data.len() {
        // stream padding is zero bytes in multiples of four
        if data[at] == 0 && streams > 0 {
            at += 1;
            continue;
        }
        if data.len() < at + 12 || data[at..at + 6] != MAGIC {
            return Err("not an xz file".into());
        }
        streams += 1;
        let check = match data[at + 7] & 0x0F {
            0 => 0,
            1 => 4,
            4 => 8,
            10 => 32,
            _ => return Err("an xz check type that is not supported".into()),
        };
        at += 12;
        loop {
            let hb = *data.get(at).ok_or("the xz file ends early")?;
            if hb == 0 {
                // the index and footer follow: this stream is done
                let mut i = at + 1;
                let n = varint(data, &mut i)?;
                for _ in 0..n * 2 {
                    varint(data, &mut i)?;
                }
                i = (i + 3) & !3;
                at = i + 4 + 12; // CRC32 of the index, the 12-byte footer
                break;
            }
            let hsize = (hb as usize + 1) * 4;
            if data.len() < at + hsize {
                return Err("the xz file ends early".into());
            }
            let h = &data[at..at + hsize];
            let flags = h[1];
            let nfilters = (flags & 0x03) as usize + 1;
            let mut i = 2;
            if flags & 0x40 != 0 {
                varint(h, &mut i)?;
            }
            if flags & 0x80 != 0 {
                varint(h, &mut i)?;
            }
            // the filter chain: any number of simple filters (BCJ, delta), then LZMA2 last
            let mut kinds: Vec<Kind> = Vec::new();
            let mut dict = 0usize;
            for k in 0..nfilters {
                let id = varint(h, &mut i)?;
                let plen = varint(h, &mut i)? as usize;
                let props = h.get(i..i.saturating_add(plen)).ok_or("damaged xz header")?;
                i += plen;
                if k + 1 == nfilters {
                    if id != 0x21 {
                        return Err("an xz file whose last filter is not LZMA2 is not supported".into());
                    }
                    let dbyte = *props.first().ok_or("damaged xz header")? as u32;
                    if plen != 1 || dbyte > 40 {
                        return Err("damaged xz header".into());
                    }
                    dict = if dbyte == 40 { u32::MAX as usize } else { ((2 | (dbyte & 1)) as usize) << (dbyte / 2 + 11) };
                } else {
                    kinds.push(Kind::from_header(id, props)?);
                }
            }
            if dict > MAX_DICT {
                return Err(format!("the xz dictionary ({} MB) is larger than allowed", dict >> 20));
            }
            at += hsize;
            let mut chain = build_chain(&kinds, &mut *out);
            let mut d = Dict { buf: vec![0; dict.max(4096)], pos: 0, full: 0, total: 0, out: &mut chain, pending: Vec::new(), written: total, limit };
            let mut m = Model::new();
            loop {
                let c = *data.get(at).ok_or("the xz file ends early")?;
                at += 1;
                match c {
                    0 => break,
                    1 | 2 => {
                        if data.len() < at + 2 {
                            return Err("the xz file ends early".into());
                        }
                        let n = ((data[at] as usize) << 8 | data[at + 1] as usize) + 1;
                        at += 2;
                        if c == 1 {
                            d.total = 0;
                            d.full = 0;
                        }
                        let src = data.get(at..at + n).ok_or("the xz file ends early")?;
                        for &b in src {
                            d.put(b)?;
                        }
                        at += n;
                    }
                    0x80..=0xFF => {
                        if data.len() < at + 4 {
                            return Err("the xz file ends early".into());
                        }
                        let un = (((c & 0x1F) as usize) << 16) + ((data[at] as usize) << 8 | data[at + 1] as usize) + 1;
                        let pk = ((data[at + 2] as usize) << 8 | data[at + 3] as usize) + 1;
                        at += 4;
                        let mode = (c >> 5) & 3;
                        if mode == 3 {
                            d.total = 0;
                            d.full = 0;
                        }
                        if mode >= 2 {
                            let p = *data.get(at).ok_or("the xz file ends early")? as u32;
                            at += 1;
                            if p >= 225 {
                                return Err("damaged LZMA properties".into());
                            }
                            (m.lc, m.lp, m.pb) = (p % 9, (p / 9) % 5, p / 45);
                            if m.lc + m.lp > 4 {
                                return Err("LZMA properties that are not supported".into());
                            }
                        }
                        if mode >= 1 {
                            m.reset();
                        }
                        if m.lit.is_empty() {
                            return Err("damaged LZMA data".into());
                        }
                        let src = data.get(at..at + pk).ok_or("the xz file ends early")?;
                        lzma_chunk(&mut m, &mut d, src, un)?;
                        at += pk;
                    }
                    _ => return Err("damaged xz data".into()),
                }
            }
            d.flush()?;
            let written = d.written;
            drop(d);
            chain.fin().map_err(|e| e.to_string())?;
            drop(chain);
            total = written;
            // block padding to four bytes, then the check
            at = ((at + 3) & !3) + check;
        }
    }
    if streams == 0 {
        return Err("not an xz file".into());
    }
    Ok(total)
}

/// MD5 (RFC 1321) as lower-case hex. Only for checking a download against the digest a vendor publishes, never for security.
pub struct Md5 {
    s: [u32; 4],
    buf: Vec<u8>,
    len: u64,
}

const K: [u32; 64] = {
    // floor(2^32 * abs(sin(i + 1))), written out
    [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ]
};
const R: [u32; 64] = [7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21];

impl Md5 {
    pub fn new() -> Md5 {
        Md5 { s: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], buf: Vec::new(), len: 0 }
    }
    fn block(&mut self, b: &[u8]) {
        let m: Vec<u32> = b.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let [mut a, mut bb, mut c, mut d] = self.s;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((bb & c) | (!bb & d), i),
                1 => ((d & bb) | (!d & c), (5 * i + 1) % 16),
                2 => (bb ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (bb | !d), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = bb;
            bb = bb.wrapping_add(a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]).rotate_left(R[i]));
            a = t;
        }
        self.s = [self.s[0].wrapping_add(a), self.s[1].wrapping_add(bb), self.s[2].wrapping_add(c), self.s[3].wrapping_add(d)];
    }
    pub fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.buf.extend_from_slice(data);
        let n = self.buf.len() / 64 * 64;
        let full: Vec<u8> = self.buf.drain(..n).collect();
        for b in full.chunks(64) {
            self.block(b);
        }
    }
    pub fn hex(mut self) -> String {
        let bits = self.len.wrapping_mul(8);
        let mut tail = vec![0x80u8];
        while (self.buf.len() + tail.len()) % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_le_bytes());
        self.buf.extend_from_slice(&tail);
        let buf = std::mem::take(&mut self.buf);
        for b in buf.chunks(64) {
            self.block(b);
        }
        self.s.iter().flat_map(|w| w.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
    }
}

/// The MD5 for `file` in a vendor's `md5  name` file (a line with only the digest counts too).
pub fn md5_from(text: &str, file: &str) -> Option<String> {
    let valid = |h: &str| h.len() == 32 && h.chars().all(|c| c.is_ascii_hexdigit());
    let mut single = None;
    for l in text.lines() {
        let mut p = l.split_whitespace();
        let (Some(h), name) = (p.next(), p.next()) else { continue };
        if !valid(h) {
            continue;
        }
        match name {
            None => single = Some(h.to_ascii_lowercase()),
            Some(n) if n.trim_start_matches('*').rsplit('/').next() == Some(file) => return Some(h.to_ascii_lowercase()),
            _ => {}
        }
    }
    single
}

/// Unpacks a `.tar.xz` into `dest`: the xz layer into a scratch tar (removed afterwards), then the tar.
pub fn untar_xz(src: &std::path::Path, dest: &std::path::Path, scratch: &std::path::Path, max_total: u64) -> Result<crate::archive::Unpacked, String> {
    let data = std::fs::read(src).map_err(|e| e.to_string())?;
    let tar = scratch.join("archive.tar.partial");
    let r = (|| {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tar).map_err(|e| e.to_string())?);
        decompress(&data, &mut f, max_total + (64 << 20))?;
        f.flush().map_err(|e| e.to_string())?;
        drop(f);
        crate::archive::untar_file(&tar, dest)
    })();
    let _ = std::fs::remove_file(&tar);
    r
}

/// For tests (here and in `clis.rs`): an `.xz` file that holds `data` in uncompressed LZMA2 chunks (valid format, no compression).
#[cfg(test)]
pub(crate) fn make_xz_stored(data: &[u8]) -> Vec<u8> {
    let mut x = vec![0xFD, b'7', b'z', b'X', b'Z', 0, 0, 4, 0, 0, 0, 0];
    x.extend_from_slice(&[2, 0, 0x21, 1, 0x16, 0, 0, 0, 0, 0, 0, 0]);
    for (i, c) in data.chunks(65536).enumerate() {
        x.push(if i == 0 { 1 } else { 2 });
        x.extend_from_slice(&((c.len() - 1) as u16).to_be_bytes());
        x.extend_from_slice(c);
    }
    x.push(0);
    while x.len() % 4 != 0 {
        x.push(0);
    }
    x.extend_from_slice(&[0; 8]); // the check
    x.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // index: indicator, one record (sizes not read), padding
    x.extend_from_slice(&[0; 4]);
    x.extend_from_slice(&[0; 12]);
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_known_values() {
        let h = |s: &str| {
            let mut m = Md5::new();
            m.update(s.as_bytes());
            m.hex()
        };
        assert_eq!(h(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(h("abc"), "900150983cd24fb0d6963f7d28e17f72");
        let long = "12345678901234567890123456789012345678901234567890123456789012345678901234567890";
        assert_eq!(h(long), "57edf4a22be3c955ac49da2e2107b67a");
        // split updates give the same answer
        let mut m = Md5::new();
        m.update(&long.as_bytes()[..13]);
        m.update(&long.as_bytes()[13..]);
        assert_eq!(m.hex(), "57edf4a22be3c955ac49da2e2107b67a");
    }

    #[test]
    fn reads_the_vendors_md5_file() {
        let t = "7FA72B652E19BF84C9461E332EA1CDF3  ffmpeg-release-amd64-static.tar.xz\n";
        assert_eq!(md5_from(t, "ffmpeg-release-amd64-static.tar.xz").as_deref(), Some("7fa72b652e19bf84c9461e332ea1cdf3"));
        assert_eq!(md5_from(t, "other.tar.xz"), None);
        assert_eq!(md5_from("not a digest\n", "x"), None);
    }

    #[test]
    fn refuses_what_is_not_xz() {
        let mut out = Vec::new();
        assert!(decompress(b"hello world, not xz", &mut out, 1 << 20).is_err());
        assert!(decompress(b"", &mut out, 1 << 20).is_err());
    }

    #[test]
    fn unpacks_a_small_file() {
        const XZ: &[u8] = &[0xfd,0x37,0x7a,0x58,0x5a,0x00,0x00,0x04,0xe6,0xd6,0xb4,0x46,0x04,0xc0,0x8b,0x02,0xcc,0x0d,0x21,0x01,0x1c,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x0f,0x6d,0x2b,0x12,0xe0,0x06,0xcb,0x01,0x03,0x5d,0x00,0x34,0x19,0x49,0xee,0x8d,0xe9,0x13,0xb8,0xaf,0x95,0x7c,0x89,0x50,0x59,0x3e,0xc0,0x03,0xe4,0x4d,0x99,0x3f,0xfc,0x73,0x51,0x64,0x77,0x1b,0xd7,0x99,0x69,0x9d,0x86,0x32,0x9b,0xda,0xe2,0x6b,0x81,0x7f,0x97,0x97,0x16,0x05,0x0b,0x0e,0x80,0xa3,0xee,0x80,0xce,0x0d,0xe2,0x71,0x41,0x12,0x4c,0x35,0xc5,0x5e,0x34,0x7f,0xcf,0xdb,0x3f,0x4d,0x6e,0x5e,0xa5,0x92,0xbe,0xcf,0xc8,0x3f,0x50,0x0a,0x66,0xa9,0xfc,0xaa,0x20,0x4e,0x87,0x96,0x2a,0x3e,0xed,0xc9,0xee,0x8b,0x42,0x5d,0xe5,0x29,0x82,0x0d,0x26,0x05,0xe9,0x27,0x74,0xa4,0x79,0x99,0xe6,0x4a,0xc5,0x05,0x36,0xd1,0x07,0xaa,0xbd,0xda,0xfd,0x34,0xc6,0x7e,0x0c,0xb9,0x11,0xbd,0x21,0xd8,0xa3,0xc9,0xd7,0x47,0xc7,0x27,0x8e,0x6a,0xd8,0x60,0xe9,0x0b,0xe3,0x4b,0x83,0x46,0x06,0x3b,0x0d,0x5b,0x47,0x0b,0xff,0xe1,0xb7,0x15,0x09,0x2b,0xce,0x99,0x05,0x46,0x42,0x7c,0xc4,0x23,0x96,0x15,0x35,0x71,0x9b,0x03,0xb8,0xa8,0x44,0xb9,0xb2,0xd3,0xe9,0x02,0xda,0xbb,0x9e,0x27,0xe7,0x5e,0xf5,0xd6,0x9a,0x52,0x14,0x82,0xcc,0xe8,0xaa,0x92,0xf7,0x37,0xaf,0x59,0x90,0x0c,0xff,0x12,0x7f,0x70,0xc0,0xb1,0x03,0x39,0x3f,0xe0,0x25,0x61,0xda,0x03,0xb2,0xb7,0xd3,0x9f,0xb3,0xe3,0xa4,0xc3,0xf8,0x27,0x64,0x14,0xdf,0x87,0xbd,0xbb,0xc6,0x5c,0xc3,0xc4,0xf4,0x39,0xb3,0x81,0x82,0xea,0x57,0x4b,0xcd,0x32,0xe8,0x91,0x02,0xfc,0x02,0x76,0x79,0x88,0xca,0x6f,0x11,0x05,0x5e,0xe3,0x78,0xff,0x84,0x1b,0xea,0x00,0x00,0x00,0x24,0x4a,0xd8,0xfc,0xff,0xb0,0x7a,0xf7,0x00,0x01,0xa7,0x02,0xcc,0x0d,0x00,0x00,0x06,0x64,0xcf,0x89,0xb1,0xc4,0x67,0xfb,0x02,0x00,0x00,0x00,0x00,0x04,0x59,0x5a];
        let want: String = "hello iyakki ".repeat(50) + &(0..300).map(|i| format!("{i},")).collect::<String>();
        let mut out = Vec::new();
        let n = decompress(XZ, &mut out, 1 << 20).unwrap();
        assert_eq!(n as usize, want.len());
        assert_eq!(String::from_utf8(out).unwrap(), want);
        // the same file cut short is an error, and a tiny limit stops it
        assert!(decompress(&XZ[..XZ.len() / 2], &mut Vec::new(), 1 << 20).is_err());
        assert!(decompress(XZ, &mut Vec::new(), 100).is_err());
    }

    /// Compress `data` with the `xz` program and the given filter options (`None` when xz or the filter is not on this machine).
    fn xz_with(args: &[&str], data: &[u8]) -> Option<Vec<u8>> {
        use std::process::{Command, Stdio};
        let mut c = Command::new("xz").args(["-c", "-T1", "--check=crc32"]).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        let mut stdin = c.stdin.take()?;
        let d = data.to_vec();
        let t = std::thread::spawn(move || {
            let _ = stdin.write_all(&d);
        });
        let out = c.wait_with_output().ok()?;
        let _ = t.join();
        (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
    }

    /// Bytes that look like code for every converter: calls and jumps with small and large offsets, plus text.
    fn codeish(n: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(n);
        let mut x = 0x1234_5678u32;
        while v.len() < n {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            match (x >> 24) % 9 {
                0 => v.extend_from_slice(&[0xE8, (x >> 8) as u8, (x >> 16) as u8, 0x00, 0x00]),
                1 => v.extend_from_slice(&[0xE9, (x >> 8) as u8, 0xFF, 0xFF, 0xFF]),
                2 => v.extend_from_slice(&[(x >> 8) as u8, (x >> 16) as u8, (x >> 4) as u8, 0xEB]),
                3 => v.extend_from_slice(&[0x94, 0x00, (x >> 8) as u8, (x >> 12) as u8]),
                4 => v.extend_from_slice(&[0x90, 0x01, (x >> 8) as u8, 0xB0]),
                5 => v.extend_from_slice(&[0x48, (x >> 8) as u8, (x >> 16) as u8, 0x01]),
                6 => v.extend_from_slice(&[0x40, 0x00, (x >> 8) as u8, (x >> 16) as u8]),
                7 => v.extend_from_slice(&[(x >> 8) as u8, 0xF0, (x >> 16) as u8, 0xF8]),
                _ => v.extend_from_slice(b"static 7-Zip program text "),
            }
        }
        v.truncate(n);
        v
    }

    #[test]
    fn every_branch_filter_xz_writes_round_trips_through_our_decoder() {
        if xz_with(&[], b"x").is_none() {
            eprintln!("skipped: no xz program");
            return;
        }
        let data = codeish(300_000);
        let mut ran = 0;
        for f in ["--x86", "--arm", "--armthumb", "--powerpc", "--sparc", "--arm64", "--delta=dist=4", "--delta"] {
            let Some(x) = xz_with(&[f, "--lzma2=preset=1"], &data) else {
                eprintln!("skipped {f}: this xz does not have it");
                continue;
            };
            let mut out = Vec::new();
            decompress(&x, &mut out, 1 << 24).unwrap_or_else(|e| panic!("{f}: {e}"));
            assert!(out == data, "{f}: the bytes differ");
            ran += 1;
        }
        assert!(ran >= 4, "at least the common filters ran ({ran})");
        // two filters in one chain (delta then x86), a filter over several blocks, and a tiny input
        if let Some(x) = xz_with(&["--x86", "--delta=dist=2", "--lzma2=preset=0"], &data) {
            let mut out = Vec::new();
            decompress(&x, &mut out, 1 << 24).unwrap();
            assert!(out == data, "x86 + delta");
        }
        if let Some(x) = xz_with(&["--x86", "--lzma2=preset=1", "--block-size=40000"], &data) {
            let mut out = Vec::new();
            decompress(&x, &mut out, 1 << 24).unwrap();
            assert!(out == data, "x86 over many blocks");
        }
        let tiny = [0xE8u8, 1, 2, 0, 0, 7];
        if let Some(x) = xz_with(&["--x86", "--lzma2=preset=0"], &tiny) {
            let mut out = Vec::new();
            decompress(&x, &mut out, 1 << 20).unwrap();
            assert_eq!(out, tiny);
        }
    }

    /// Replace the filter list of the first block header of a plain LZMA2 file (made by `make_xz_stored`) and fix the sizes.
    fn with_filters(filters: &[u8], count: u8) -> Vec<u8> {
        // header: size byte, flags, filters..., padding, crc32 (not read)
        let mut hdr = vec![0u8, count - 1];
        hdr.extend_from_slice(filters);
        while (hdr.len() + 4) % 4 != 0 {
            hdr.push(0);
        }
        hdr.extend_from_slice(&[0; 4]);
        hdr[0] = (hdr.len() / 4 - 1) as u8;
        let mut x = vec![0xFD, b'7', b'z', b'X', b'Z', 0, 0, 4, 0, 0, 0, 0];
        x.extend_from_slice(&hdr);
        x.extend_from_slice(&[1, 0, 4, b'a', b'b', b'c', b'd', b'e', 0]);
        while x.len() % 4 != 0 {
            x.push(0);
        }
        x.extend_from_slice(&[0; 8]);
        x.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        x
    }

    #[test]
    fn hostile_filter_lists_are_refused_plainly_and_a_good_one_works() {
        let lz = [0x21u8, 1, 0x16];
        let mut out = Vec::new();
        // x86 then LZMA2: fine
        let ok = with_filters(&[&[0x04u8, 0][..], &lz].concat(), 2);
        assert_eq!(decompress(&ok, &mut out, 1 << 20).unwrap(), 5);
        assert_eq!(out, b"abcde");
        let bad: Vec<(Vec<u8>, u8, &str)> = vec![
            ([&[0x06u8, 0][..], &lz].concat(), 2, "IA-64"),
            ([&[0x0Bu8, 0][..], &lz].concat(), 2, "RISC-V"),
            ([&[0x55u8, 0][..], &lz].concat(), 2, "not supported"),
            ([&[0x04u8, 4, 1, 2, 3, 4][..], &lz].concat(), 2, "options"),
            ([&[0x03u8, 0][..], &lz].concat(), 2, "damaged"),
            ([&[0x04u8, 0][..], &[0x04, 0]].concat(), 2, "last filter"),
            ([&[0x21u8, 1, 0x16][..], &lz].concat(), 2, "damaged"),
            ([&[0x04u8, 9][..], &lz].concat(), 2, "damaged"),
            ([&[0x04u8][..]].concat(), 2, "last filter"),
        ];
        for (f, n, why) in bad {
            let x = with_filters(&f, n);
            let e = decompress(&x, &mut Vec::new(), 1 << 20).unwrap_err();
            assert!(e.contains(why), "{why}: {e}");
        }
        // a chain cut anywhere never panics
        for cut in 0..ok.len() {
            let _ = decompress(&ok[..cut], &mut Vec::new(), 1 << 20);
        }
        // the output limit holds through a filter
        let big = with_filters(&[&[0x04u8, 0][..], &lz].concat(), 2);
        assert!(decompress(&big, &mut Vec::new(), 2).is_err());
    }

    /// Set IYAKKI_XZ_FILE to an .xz file and IYAKKI_XZ_MD5 to the md5 of what it unpacks to.
    #[test]
    #[ignore]
    fn real_file() {
        let (Ok(p), Ok(want)) = (std::env::var("IYAKKI_XZ_FILE"), std::env::var("IYAKKI_XZ_MD5")) else { return };
        let data = std::fs::read(p).unwrap();
        let mut out = Vec::new();
        decompress(&data, &mut out, 1 << 30).unwrap();
        let mut m = Md5::new();
        m.update(&out);
        assert_eq!(m.hex(), want);
    }
}
