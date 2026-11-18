//! The Bluetooth audio codec SBC (the low-complexity subband codec every A2DP device must support), written in-house from the
//! A2DP specification's description: no library, no dependency. An encoder and a decoder for every mode: 4 or 8 subbands,
//! 4, 8, 12 or 16 blocks, mono, dual channel, stereo and joint stereo, loudness or SNR bit allocation, any bitpool.
//!
//! Frame: sync byte 0x9C, a byte with the sampling frequency, block count, channel mode, allocation method and subband count, the
//! bitpool, a CRC-8 of the header and scale factors, then (joint stereo) one flag per subband, the 4-bit scale factors and the
//! quantised samples. The filter bank is the cosine-modulated 4 or 8 band bank of the specification; its window tables are in
//! [`crate::sbc_tables`].
//!
//! Hostile input: [`Decoder::decode`] checks the sync byte, the parameter limits, the length and the CRC before it decodes, and never
//! reads past the frame.
// SPDX-License-Identifier: Apache-2.0

use crate::sbc_tables::{D4, D8};

pub const SYNC: u8 = 0x9c;
const FREQS: [u32; 4] = [16000, 32000, 44100, 48000];
const BLOCKS: [u8; 4] = [4, 8, 12, 16];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelMode {
    Mono,
    Dual,
    Stereo,
    Joint,
}

impl ChannelMode {
    fn code(self) -> u8 {
        match self {
            ChannelMode::Mono => 0,
            ChannelMode::Dual => 1,
            ChannelMode::Stereo => 2,
            ChannelMode::Joint => 3,
        }
    }
    pub fn channels(self) -> usize {
        if self == ChannelMode::Mono { 1 } else { 2 }
    }
}

/// The settings of a stream (they are repeated in every frame header).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// index into 16, 32, 44.1, 48 kHz
    pub freq: u8,
    /// 4, 8, 12 or 16
    pub blocks: u8,
    pub mode: ChannelMode,
    /// loudness allocation (true) or SNR allocation (false)
    pub loudness: bool,
    /// 4 or 8
    pub subbands: u8,
    pub bitpool: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SbcError {
    TooShort,
    BadSync,
    BadParams(&'static str),
    BadCrc,
}

impl std::fmt::Display for SbcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SbcError::TooShort => write!(f, "the audio frame is cut short"),
            SbcError::BadSync => write!(f, "not an audio frame (no sync byte)"),
            SbcError::BadParams(m) => write!(f, "the audio frame has settings that are not allowed ({m})"),
            SbcError::BadCrc => write!(f, "the audio frame is damaged (checksum)"),
        }
    }
}

impl std::error::Error for SbcError {}

impl Params {
    pub fn sample_rate(&self) -> u32 {
        FREQS[(self.freq & 3) as usize]
    }
    pub fn channels(&self) -> usize {
        self.mode.channels()
    }
    /// PCM samples per channel in one frame.
    pub fn frame_samples(&self) -> usize {
        self.blocks as usize * self.subbands as usize
    }

    /// The largest bitpool the specification allows for this mode and subband count.
    pub fn max_bitpool(&self) -> u32 {
        let per = 16 * self.subbands as u32;
        if matches!(self.mode, ChannelMode::Mono | ChannelMode::Dual) { per } else { 2 * per }
    }

    pub fn validate(&self) -> Result<(), SbcError> {
        if self.freq > 3 {
            return Err(SbcError::BadParams("sampling frequency"));
        }
        if !BLOCKS.contains(&self.blocks) {
            return Err(SbcError::BadParams("block count"));
        }
        if self.subbands != 4 && self.subbands != 8 {
            return Err(SbcError::BadParams("subband count"));
        }
        if self.bitpool < 2 || self.bitpool as u32 > self.max_bitpool() {
            return Err(SbcError::BadParams("bitpool"));
        }
        Ok(())
    }

    /// Bytes in one frame.
    pub fn frame_len(&self) -> usize {
        let (sb, ch, bl, bp) = (self.subbands as usize, self.channels(), self.blocks as usize, self.bitpool as usize);
        let join = if self.mode == ChannelMode::Joint { sb } else { 0 };
        let head = (4 * sb * ch + join).div_ceil(8);
        let audio = match self.mode {
            ChannelMode::Mono | ChannelMode::Dual => (bl * ch * bp).div_ceil(8),
            _ => (bl * bp).div_ceil(8),
        };
        4 + head + audio
    }

    fn header_bytes(&self) -> [u8; 3] {
        let b1 = (self.freq << 6) | ((BLOCKS.iter().position(|b| *b == self.blocks).unwrap_or(3) as u8) << 4) | (self.mode.code() << 2) | ((!self.loudness as u8) << 1) | (self.subbands == 8) as u8;
        [SYNC, b1, self.bitpool]
    }
}

/// Reads the four header bytes. Does not check the CRC.
pub fn parse_header(d: &[u8]) -> Result<Params, SbcError> {
    if d.len() < 4 {
        return Err(SbcError::TooShort);
    }
    if d[0] != SYNC {
        return Err(SbcError::BadSync);
    }
    let b = d[1];
    let p = Params {
        freq: b >> 6,
        blocks: BLOCKS[((b >> 4) & 3) as usize],
        mode: match (b >> 2) & 3 {
            0 => ChannelMode::Mono,
            1 => ChannelMode::Dual,
            2 => ChannelMode::Stereo,
            _ => ChannelMode::Joint,
        },
        loudness: b & 2 == 0,
        subbands: if b & 1 == 1 { 8 } else { 4 },
        bitpool: d[2],
    };
    p.validate()?;
    Ok(p)
}

// ------------------------------------------------------------------------------------------------ bits and checksum

struct BitWriter {
    out: Vec<u8>,
    nbits: usize,
}

impl BitWriter {
    fn new() -> BitWriter {
        BitWriter { out: Vec::new(), nbits: 0 }
    }
    fn put(&mut self, value: u32, bits: u32) {
        for i in (0..bits).rev() {
            if self.nbits % 8 == 0 {
                self.out.push(0);
            }
            if (value >> i) & 1 == 1 {
                *self.out.last_mut().unwrap() |= 0x80 >> (self.nbits % 8);
            }
            self.nbits += 1;
        }
    }
}

struct BitReader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl BitReader<'_> {
    fn get(&mut self, bits: u32) -> u32 {
        let mut v = 0;
        for _ in 0..bits {
            let byte = self.d.get(self.pos / 8).copied().unwrap_or(0);
            v = (v << 1) | ((byte >> (7 - self.pos % 8)) & 1) as u32;
            self.pos += 1;
        }
        v
    }
}

/// CRC-8, polynomial x^8+x^4+x^3+x^2+1 (0x1D), start value 0x0F, over `nbits` bits of `data`, most significant bit first.
pub fn crc8(data: &[u8], nbits: usize) -> u8 {
    let mut crc = 0x0fu8;
    for i in 0..nbits {
        let bit = (data[i / 8] >> (7 - i % 8)) & 1;
        let top = (crc >> 7) ^ bit;
        crc <<= 1;
        if top == 1 {
            crc ^= 0x1d;
        }
    }
    crc
}

/// The checksum of a frame: header bytes 1 and 2, then (joint stereo) the join flags, then the scale factors.
fn frame_crc(d: &[u8], p: &Params) -> u8 {
    let (sb, ch) = (p.subbands as usize, p.channels());
    let join = if p.mode == ChannelMode::Joint { sb } else { 0 };
    let nbits = 16 + join + 4 * sb * ch;
    let mut buf = vec![d[1], d[2]];
    buf.extend_from_slice(&d[4..4 + (join + 4 * sb * ch).div_ceil(8)]);
    // bits after the scale factors in the last byte do not belong to the checksum: crc8 takes exactly nbits
    crc8(&buf, nbits)
}

// ------------------------------------------------------------------------------------------------ bit allocation

const OFFSET4: [[i32; 4]; 4] = [[-1, 0, 0, 0], [-2, 0, 0, 1], [-2, 0, 0, 1], [-2, 0, 0, 1]];
const OFFSET8: [[i32; 8]; 4] = [[-2, 0, 0, 0, 0, 0, 0, 1], [-3, 0, 0, 0, 0, 0, 1, 2], [-4, 0, 0, 0, 0, 0, 1, 2], [-4, 0, 0, 0, 0, 0, 1, 2]];

/// How many bits each subband's samples get, from the scale factors (the specification's bit allocation).
pub fn allocate(p: &Params, sf: &[[u8; 8]; 2]) -> [[u8; 8]; 2] {
    let (nsb, nch) = (p.subbands as usize, p.channels());
    let mut bitneed = [[0i32; 8]; 2];
    for ch in 0..nch {
        for sb in 0..nsb {
            bitneed[ch][sb] = if !p.loudness {
                sf[ch][sb] as i32
            } else if sf[ch][sb] == 0 {
                -5
            } else {
                let off = if nsb == 4 { OFFSET4[(p.freq & 3) as usize][sb] } else { OFFSET8[(p.freq & 3) as usize][sb] };
                let l = sf[ch][sb] as i32 - off;
                if l > 0 { l / 2 } else { l }
            };
        }
    }
    let mut bits = [[0u8; 8]; 2];
    let bitpool = p.bitpool as i32;
    // dual channel and mono: each channel has its own pool; stereo and joint: one pool over both
    let groups: Vec<Vec<usize>> = if matches!(p.mode, ChannelMode::Mono | ChannelMode::Dual) { (0..nch).map(|c| vec![c]).collect() } else { vec![vec![0, 1]] };
    for g in groups {
        let max_need = g.iter().flat_map(|&c| bitneed[c][..nsb].iter()).copied().max().unwrap_or(0);
        let min_need = g.iter().flat_map(|&c| bitneed[c][..nsb].iter()).copied().min().unwrap_or(0);
        let (mut bitcount, mut slicecount, mut bitslice) = (0i32, 0i32, max_need + 1);
        loop {
            bitslice -= 1;
            bitcount += slicecount;
            slicecount = 0;
            for &c in &g {
                for sb in 0..nsb {
                    let n = bitneed[c][sb];
                    if n > bitslice + 1 && n < bitslice + 16 {
                        slicecount += 1;
                    } else if n == bitslice + 1 {
                        slicecount += 2;
                    }
                }
            }
            // a pool larger than everything the subbands can take would never be filled: stop below the lowest need
            if bitcount + slicecount >= bitpool || bitslice < min_need - 17 {
                break;
            }
        }
        if bitcount + slicecount == bitpool {
            bitcount += slicecount;
            bitslice -= 1;
        }
        for &c in &g {
            for sb in 0..nsb {
                bits[c][sb] = if bitneed[c][sb] < bitslice + 2 { 0 } else { (bitneed[c][sb] - bitslice).min(16) as u8 };
            }
        }
        // hand out what is left: first to subbands that already have 2 to 15 bits, then to any below 16
        let n = g.len();
        for i in 0..n * nsb {
            if bitcount >= bitpool {
                break;
            }
            let (c, sb) = (g[i % n], i / n);
            if bits[c][sb] >= 2 && bits[c][sb] < 16 {
                bits[c][sb] += 1;
                bitcount += 1;
            } else if bitneed[c][sb] == bitslice + 1 && bitpool > bitcount + 1 {
                bits[c][sb] = 2;
                bitcount += 2;
            }
        }
        for i in 0..n * nsb {
            if bitcount >= bitpool {
                break;
            }
            let (c, sb) = (g[i % n], i / n);
            if bits[c][sb] < 16 {
                bits[c][sb] += 1;
                bitcount += 1;
            }
        }
    }
    bits
}

// ------------------------------------------------------------------------------------------------ the filter bank

/// The window in use (see [`crate::sbc_tables`]: the designed one, or the legacy one when the cargo feature `legacy-sbc-window` is on).
fn window(nsb: usize) -> &'static [f64] {
    if nsb == 4 { &D4 } else { &D8 }
}

/// Synthesis: subband samples of one block to `nsb` PCM samples.
struct Synth {
    v: Vec<f64>,
}

impl Synth {
    fn new(nsb: usize) -> Synth {
        Synth { v: vec![0.0; 20 * nsb] }
    }
    fn block(&mut self, nsb: usize, d: &[f64], s: &[f64], out: &mut [f64]) {
        let v = &mut self.v;
        v.copy_within(0..18 * nsb, 2 * nsb);
        for i in 0..2 * nsb {
            let mut acc = 0.0;
            for (k, sk) in s.iter().enumerate().take(nsb) {
                acc += (((k as f64 + 0.5) * (i as f64 + nsb as f64 / 2.0)) * std::f64::consts::PI / nsb as f64).cos() * sk;
            }
            v[i] = acc;
        }
        for (j, o) in out.iter_mut().enumerate().take(nsb) {
            let mut x = 0.0;
            for m in 0..10 {
                let u = j + nsb * m;
                // U[i*2nsb + j'] = V[i*4nsb + j'] for j' < nsb; U[i*2nsb + nsb + j'] = V[i*4nsb + 3nsb + j']
                let (i2, r) = (u / (2 * nsb), u % (2 * nsb));
                let vi = if r < nsb { i2 * 4 * nsb + r } else { i2 * 4 * nsb + 3 * nsb + (r - nsb) };
                x += v[vi] * d[u];
            }
            *o = x;
        }
    }
}

/// Analysis: `nsb` new PCM samples of one block to subband samples.
struct Analysis {
    x: Vec<f64>,
}

impl Analysis {
    fn new(nsb: usize) -> Analysis {
        Analysis { x: vec![0.0; 10 * nsb] }
    }
    fn block(&mut self, nsb: usize, d: &[f64], pcm: &[f64], out: &mut [f64]) {
        let x = &mut self.x;
        x.copy_within(0..9 * nsb, nsb);
        for i in 0..nsb {
            // the newest sample goes to index 0
            x[nsb - 1 - i] = pcm[i];
        }
        let mut y = vec![0.0; 2 * nsb];
        for (i, yi) in y.iter_mut().enumerate() {
            for j in 0..5 {
                let idx = i + 2 * nsb * j;
                // the analysis window is the synthesis window divided by the subband count, with the opposite sign
                *yi -= x[idx] * d[idx] / nsb as f64;
            }
        }
        for (k, o) in out.iter_mut().enumerate().take(nsb) {
            let mut acc = 0.0;
            for (i, yi) in y.iter().enumerate() {
                acc += (((k as f64 + 0.5) * (i as f64 - nsb as f64 / 2.0)) * std::f64::consts::PI / nsb as f64).cos() * yi;
            }
            *o = acc;
        }
    }
}

// ------------------------------------------------------------------------------------------------ decoder

/// Decodes frames to interleaved 16-bit PCM. Keeps the filter state between frames; a change of channel count or subbands resets it.
pub struct Decoder {
    synth: [Synth; 2],
    nsb: usize,
    nch: usize,
}

/// One decoded frame.
pub struct Decoded {
    pub params: Params,
    /// bytes the frame took
    pub used: usize,
    /// interleaved samples
    pub pcm: Vec<i16>,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::new()
    }
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder { synth: [Synth::new(8), Synth::new(8)], nsb: 8, nch: 2 }
    }

    pub fn decode(&mut self, d: &[u8]) -> Result<Decoded, SbcError> {
        let p = parse_header(d)?;
        let len = p.frame_len();
        if d.len() < len {
            return Err(SbcError::TooShort);
        }
        let d = &d[..len];
        if frame_crc(d, &p) != d[3] {
            return Err(SbcError::BadCrc);
        }
        let (nsb, nch) = (p.subbands as usize, p.channels());
        if nsb != self.nsb || nch != self.nch {
            self.synth = [Synth::new(nsb), Synth::new(nsb)];
            self.nsb = nsb;
            self.nch = nch;
        }
        let mut r = BitReader { d, pos: 32 };
        let mut join = [false; 8];
        if p.mode == ChannelMode::Joint {
            for j in join.iter_mut().take(nsb) {
                *j = r.get(1) == 1;
            }
        }
        let mut sf = [[0u8; 8]; 2];
        for c in sf.iter_mut().take(nch) {
            for s in c.iter_mut().take(nsb) {
                *s = r.get(4) as u8;
            }
        }
        let bits = allocate(&p, &sf);
        let blocks = p.blocks as usize;
        // subband samples [block][channel][subband]
        let mut sbs = vec![[[0.0f64; 8]; 2]; blocks];
        for blk in sbs.iter_mut() {
            for ch in 0..nch {
                for sb in 0..nsb {
                    let b = bits[ch][sb] as u32;
                    if b == 0 {
                        continue;
                    }
                    let levels = ((1u64 << b) - 1) as f64;
                    let q = r.get(b) as f64;
                    blk[ch][sb] = ((2.0 * q + 1.0) / levels - 1.0) * (1u64 << (sf[ch][sb] + 1)) as f64;
                }
            }
            if p.mode == ChannelMode::Joint {
                for sb in 0..nsb {
                    if join[sb] {
                        let (m, s) = (blk[0][sb], blk[1][sb]);
                        blk[0][sb] = m + s;
                        blk[1][sb] = m - s;
                    }
                }
            }
        }
        let mut pcm = vec![0i16; blocks * nsb * nch];
        let mut out = [0.0f64; 8];
        for (b, blk) in sbs.iter().enumerate() {
            for ch in 0..nch {
                self.synth[ch].block(nsb, window(nsb), &blk[ch][..nsb], &mut out[..nsb]);
                for j in 0..nsb {
                    pcm[(b * nsb + j) * nch + ch] = out[j].round().clamp(-32768.0, 32767.0) as i16;
                }
            }
        }
        Ok(Decoded { params: p, used: len, pcm })
    }
}

// ------------------------------------------------------------------------------------------------ encoder

/// Encodes interleaved 16-bit PCM, one frame at a time.
pub struct Encoder {
    p: Params,
    an: [Analysis; 2],
}

impl Encoder {
    pub fn new(p: Params) -> Result<Encoder, SbcError> {
        p.validate()?;
        let n = p.subbands as usize;
        Ok(Encoder { p, an: [Analysis::new(n), Analysis::new(n)] })
    }

    pub fn params(&self) -> &Params {
        &self.p
    }

    /// Interleaved samples one frame takes (all channels).
    pub fn frame_input_len(&self) -> usize {
        self.p.frame_samples() * self.p.channels()
    }

    /// Encodes exactly [`frame_input_len`](Self::frame_input_len) samples.
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>, SbcError> {
        let p = self.p;
        let (nsb, nch, blocks) = (p.subbands as usize, p.channels(), p.blocks as usize);
        if pcm.len() != self.frame_input_len() {
            return Err(SbcError::TooShort);
        }
        let mut sbs = vec![[[0.0f64; 8]; 2]; blocks];
        for (b, blk) in sbs.iter_mut().enumerate() {
            for ch in 0..nch {
                let inp: Vec<f64> = (0..nsb).map(|j| pcm[(b * nsb + j) * nch + ch] as f64).collect();
                let mut o = [0.0f64; 8];
                self.an[ch].block(nsb, window(nsb), &inp, &mut o[..nsb]);
                blk[ch] = o;
            }
        }
        let scale = |sbs: &[[[f64; 8]; 2]], ch: usize, sb: usize| -> u8 {
            let m = sbs.iter().map(|b| b[ch][sb].abs()).fold(0.0, f64::max);
            let mut sf = 0u8;
            while sf < 15 && ((1u64 << (sf + 1)) as f64) < m {
                sf += 1;
            }
            sf
        };
        let mut sf = [[0u8; 8]; 2];
        for ch in 0..nch {
            for sb in 0..nsb {
                sf[ch][sb] = scale(&sbs, ch, sb);
            }
        }
        let mut join = [false; 8];
        if p.mode == ChannelMode::Joint {
            // mid/side where it needs fewer bits; the last subband is never joined
            let mut ms = sbs.clone();
            for blk in ms.iter_mut() {
                for sb in 0..nsb {
                    let (l, r) = (blk[0][sb], blk[1][sb]);
                    blk[0][sb] = (l + r) / 2.0;
                    blk[1][sb] = (l - r) / 2.0;
                }
            }
            for sb in 0..nsb - 1 {
                let (a, b) = (scale(&ms, 0, sb), scale(&ms, 1, sb));
                if (a as u32 + b as u32) < (sf[0][sb] as u32 + sf[1][sb] as u32) {
                    join[sb] = true;
                    sf[0][sb] = a;
                    sf[1][sb] = b;
                    for (blk, m) in sbs.iter_mut().zip(ms.iter()) {
                        blk[0][sb] = m[0][sb];
                        blk[1][sb] = m[1][sb];
                    }
                }
            }
        }
        let bits = allocate(&p, &sf);
        let mut w = BitWriter::new();
        let h = p.header_bytes();
        w.put(h[0] as u32, 8);
        w.put(h[1] as u32, 8);
        w.put(h[2] as u32, 8);
        w.put(0, 8); // checksum, filled in below
        if p.mode == ChannelMode::Joint {
            for j in join.iter().take(nsb) {
                w.put(*j as u32, 1);
            }
        }
        for c in sf.iter().take(nch) {
            for s in c.iter().take(nsb) {
                w.put(*s as u32, 4);
            }
        }
        for blk in &sbs {
            for ch in 0..nch {
                for sb in 0..nsb {
                    let b = bits[ch][sb] as u32;
                    if b == 0 {
                        continue;
                    }
                    let levels = ((1u64 << b) - 1) as f64;
                    let x = blk[ch][sb] / (1u64 << (sf[ch][sb] + 1)) as f64;
                    let q = (((x + 1.0) * levels / 2.0).floor()).clamp(0.0, levels - 1.0).max(0.0);
                    w.put(q as u32, b);
                }
            }
        }
        let mut out = w.out;
        out.resize(p.frame_len(), 0);
        out[3] = frame_crc(&out, &p);
        Ok(out)
    }
}

/// Builds a frame from explicit scale factors, join flags and quantised values (used by the tests).
/// `q[block][channel][subband]`.
pub fn pack_frame(p: &Params, sf: &[[u8; 8]; 2], join: &[bool; 8], q: &[[[u32; 8]; 2]]) -> Vec<u8> {
    let (nsb, nch) = (p.subbands as usize, p.channels());
    let bits = allocate(p, sf);
    let mut w = BitWriter::new();
    let h = p.header_bytes();
    for x in h {
        w.put(x as u32, 8);
    }
    w.put(0, 8);
    if p.mode == ChannelMode::Joint {
        for j in join.iter().take(nsb) {
            w.put(*j as u32, 1);
        }
    }
    for c in sf.iter().take(nch) {
        for s in c.iter().take(nsb) {
            w.put(*s as u32, 4);
        }
    }
    for blk in q.iter().take(p.blocks as usize) {
        for ch in 0..nch {
            for sb in 0..nsb {
                let b = bits[ch][sb] as u32;
                if b > 0 {
                    w.put(blk[ch][sb], b);
                }
            }
        }
    }
    let mut out = w.out;
    out.resize(p.frame_len(), 0);
    out[3] = frame_crc(&out, p);
    out
}

#[cfg(test)]
#[path = "sbc_tests.rs"]
mod tests;

/// Runs `pcm` (mono, a whole number of blocks) through the analysis and then the synthesis filter bank with the window `da` for the analysis and `ds` for the synthesis, with no
/// quantisation in between: what the filter bank alone does to a signal. Used to design and to check window tables ([`crate::sbc_design`]).
pub(crate) fn filter_bank_loop(nsb: usize, da: &[f64], ds: &[f64], pcm: &[f64]) -> Vec<f64> {
    let (mut an, mut sy) = (Analysis::new(nsb), Synth::new(nsb));
    let mut out = Vec::with_capacity(pcm.len());
    let (mut s, mut o) = (vec![0.0; nsb], vec![0.0; nsb]);
    for blk in pcm.chunks_exact(nsb) {
        an.block(nsb, da, blk, &mut s);
        sy.block(nsb, ds, &s, &mut o);
        out.extend_from_slice(&o);
    }
    out
}
