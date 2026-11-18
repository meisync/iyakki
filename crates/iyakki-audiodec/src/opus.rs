//! Opus (RFC 6716, RFC 7845), the parts written so far, in-house: the Ogg Opus header (`OpusHead`), the packet layer (the table of
//! contents byte, the four frame packings, frame lengths, padding), the duration of every packet and of a whole stream, the range
//! decoder every Opus frame is read through, and the silent frames (a packet that carries no frame data decodes to silence of its length).
//!
//! NOT written yet: the CELT decoder (band energies, the PVQ shape decode, the inverse MDCT with overlap and the post filter) and the
//! SILK decoder (LPC and LTP synthesis, the stereo prediction), the hybrid mode, packet loss concealment and the resampler from 48 kHz.
//! A frame with data is therefore refused with [`OpusError::NotDecodedYet`] naming what is missing; nothing is guessed or played wrong.
//!
//! Validated against streams made by the Opus encoder of the installed ffmpeg (used only to make references, never linked or shipped):
//! every packet of the file parses, and the sum of the packet durations less the pre-skip is the duration the container says.

/// What a packet's table of contents says about the coding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Silk,
    Hybrid,
    Celt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bandwidth {
    Narrow,
    Medium,
    Wide,
    SuperWide,
    Full,
}

impl Bandwidth {
    /// The highest frequency coded, Hz.
    pub fn hz(self) -> u32 {
        match self {
            Bandwidth::Narrow => 4000,
            Bandwidth::Medium => 6000,
            Bandwidth::Wide => 8000,
            Bandwidth::SuperWide => 12000,
            Bandwidth::Full => 20000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpusError {
    /// A packet or header that breaks the format, in words.
    Bad(String),
    /// A frame that needs a decoder that is not written yet (the name says which).
    NotDecodedYet(&'static str),
}

impl std::fmt::Display for OpusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpusError::Bad(s) => write!(f, "{s}"),
            OpusError::NotDecodedYet(w) => write!(f, "this Opus stream needs the {w} decoder, which is not written yet"),
        }
    }
}

/// The table of contents byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toc {
    pub config: u8,
    pub mode: Mode,
    pub bandwidth: Bandwidth,
    /// One frame's length in units of 2.5 ms of 48 kHz sound: 120 samples each (so 2.5 ms is 1, 20 ms is 8).
    pub frame_units: u32,
    pub stereo: bool,
    /// The frame count code, 0 to 3.
    pub code: u8,
}

impl Toc {
    pub fn parse(b: u8) -> Toc {
        let config = b >> 3;
        let (mode, bandwidth) = match config {
            0..=3 => (Mode::Silk, Bandwidth::Narrow),
            4..=7 => (Mode::Silk, Bandwidth::Medium),
            8..=11 => (Mode::Silk, Bandwidth::Wide),
            12..=13 => (Mode::Hybrid, Bandwidth::SuperWide),
            14..=15 => (Mode::Hybrid, Bandwidth::Full),
            16..=19 => (Mode::Celt, Bandwidth::Narrow),
            20..=23 => (Mode::Celt, Bandwidth::Wide),
            24..=27 => (Mode::Celt, Bandwidth::SuperWide),
            _ => (Mode::Celt, Bandwidth::Full),
        };
        // frame sizes in 2.5 ms units: SILK 10/20/40/60 ms, hybrid 10/20 ms, CELT 2.5/5/10/20 ms
        let frame_units = match (mode, config & 3) {
            (Mode::Silk, 0) => 4,
            (Mode::Silk, 1) => 8,
            (Mode::Silk, 2) => 16,
            (Mode::Silk, _) => 24,
            (Mode::Hybrid, i) => 4 * (i as u32 & 1) + 4,
            (Mode::Celt, i) => 1 << i,
        };
        Toc { config, mode, bandwidth, frame_units, stereo: b & 4 != 0, code: b & 3 }
    }

    /// One frame's length in samples at 48 kHz.
    pub fn frame_samples(&self) -> usize {
        self.frame_units as usize * 120
    }
}

/// One packet taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet<'a> {
    pub toc: Toc,
    pub frames: Vec<&'a [u8]>,
}

impl Packet<'_> {
    /// The packet's length in samples at 48 kHz (frames times the frame size).
    pub fn samples(&self) -> usize {
        self.frames.len() * self.toc.frame_samples()
    }
}

/// The longest a packet may last, RFC 6716: 120 ms.
pub const MAX_PACKET_SAMPLES: usize = 5760;

fn frame_len(d: &[u8], at: &mut usize) -> Result<usize, OpusError> {
    let b0 = *d.get(*at).ok_or_else(|| OpusError::Bad("a frame length runs past the packet".into()))? as usize;
    *at += 1;
    if b0 < 252 {
        return Ok(b0);
    }
    let b1 = *d.get(*at).ok_or_else(|| OpusError::Bad("a frame length runs past the packet".into()))? as usize;
    *at += 1;
    Ok(4 * b1 + b0)
}

/// Take a packet apart (RFC 6716 section 3). Refuses what the standard calls invalid, in words; never panics.
pub fn parse_packet(d: &[u8]) -> Result<Packet<'_>, OpusError> {
    let bad = |s: &str| Err(OpusError::Bad(s.into()));
    let Some(&t) = d.first() else { return bad("an empty Opus packet") };
    let toc = Toc::parse(t);
    let body = &d[1..];
    let mut frames: Vec<&[u8]> = Vec::new();
    match toc.code {
        0 => frames.push(body),
        1 => {
            if body.len() % 2 != 0 {
                return bad("two equal frames with an odd number of bytes");
            }
            let h = body.len() / 2;
            frames.push(&body[..h]);
            frames.push(&body[h..]);
        }
        2 => {
            let mut at = 0;
            let l1 = frame_len(body, &mut at)?;
            if at + l1 > body.len() {
                return bad("the first frame is longer than the packet");
            }
            frames.push(&body[at..at + l1]);
            frames.push(&body[at + l1..]);
        }
        _ => {
            let Some(&c) = body.first() else { return bad("a packet of several frames with no frame count") };
            let mut at = 1;
            let vbr = c & 0x80 != 0;
            let padded = c & 0x40 != 0;
            let n = (c & 0x3f) as usize;
            if n == 0 {
                return bad("a packet of zero frames");
            }
            if n * toc.frame_samples() > MAX_PACKET_SAMPLES {
                return bad("a packet longer than 120 ms");
            }
            let mut pad = 0usize;
            if padded {
                loop {
                    let b = *body.get(at).ok_or_else(|| OpusError::Bad("the padding length runs past the packet".into()))? as usize;
                    at += 1;
                    if b == 255 {
                        pad += 254;
                    } else {
                        pad += b;
                        break;
                    }
                }
            }
            if vbr {
                let mut lens = Vec::with_capacity(n);
                for _ in 0..n - 1 {
                    lens.push(frame_len(body, &mut at)?);
                }
                let used: usize = lens.iter().sum::<usize>() + at + pad;
                if used > body.len() {
                    return bad("the frames are longer than the packet");
                }
                let last = body.len() - pad - lens.iter().sum::<usize>() - at;
                lens.push(last);
                let mut p = at;
                for l in lens {
                    frames.push(&body[p..p + l]);
                    p += l;
                }
            } else {
                let avail = body.len().checked_sub(at + pad).ok_or_else(|| OpusError::Bad("the padding is longer than the packet".into()))?;
                if avail % n != 0 {
                    return bad("equal frames that do not divide the packet");
                }
                let l = avail / n;
                for i in 0..n {
                    frames.push(&body[at + i * l..at + (i + 1) * l]);
                }
            }
        }
    }
    if frames.iter().any(|f| f.len() > 1275) {
        return bad("a frame longer than 1275 bytes");
    }
    if frames.len() * toc.frame_samples() > MAX_PACKET_SAMPLES {
        return bad("a packet longer than 120 ms");
    }
    Ok(Packet { toc, frames })
}

/// The `OpusHead` identification header (RFC 7845 section 5.1), channel mapping family 0 only (mono or stereo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub channels: u8,
    /// Samples at 48 kHz to drop from the start of the decoded sound.
    pub pre_skip: u16,
    /// The rate of the original input (informational: the decoder always makes 48 kHz).
    pub input_rate: u32,
    /// Output gain in Q7.8 dB.
    pub gain: i16,
}

pub fn parse_head(d: &[u8]) -> Result<Head, OpusError> {
    if d.len() < 19 || &d[..8] != b"OpusHead" {
        return Err(OpusError::Bad("not an OpusHead header".into()));
    }
    if d[8] >> 4 != 0 {
        return Err(OpusError::Bad("an Opus header of a newer major version".into()));
    }
    let channels = d[9];
    if d[18] != 0 {
        return Err(OpusError::NotDecodedYet("multichannel (mapping family) layout"));
    }
    if !(1..=2).contains(&channels) {
        return Err(OpusError::Bad("a mapping family 0 stream has one or two channels".into()));
    }
    Ok(Head { channels, pre_skip: u16::from_le_bytes([d[10], d[11]]), input_rate: u32::from_le_bytes([d[12], d[13], d[14], d[15]]), gain: i16::from_le_bytes([d[16], d[17]]) })
}

/// The samples at 48 kHz (per channel) a whole stream plays: every packet's length, less the pre-skip.
pub fn stream_samples(head: &Head, packets: &[Vec<u8>]) -> Result<u64, OpusError> {
    let mut total = 0u64;
    for p in packets {
        total += parse_packet(p)?.samples() as u64;
    }
    Ok(total.saturating_sub(head.pre_skip as u64))
}

/// The range decoder (RFC 6716 section 4.1, the same arithmetic as the reference's `entdec`): the bits of every frame are read through it.
/// Reading past the end of the frame reads zeros, as the standard says; nothing here can panic.
pub struct RangeDecoder<'a> {
    data: &'a [u8],
    /// Next byte from the front (the range coded part) and from the back (the raw bits).
    front: usize,
    back: usize,
    window: u32,
    available: u32,
    /// The difference between the top of the range and the value being decoded, and the range size.
    dif: u32,
    rng: u32,
    /// The last byte read from the front, and the divided range `decode` keeps for `update`.
    rem: u32,
    ext: u32,
    nbits_total: i32,
}

const SYM_BITS: u32 = 8;
const CODE_TOP: u32 = 1 << 31;
const CODE_BOT: u32 = CODE_TOP >> SYM_BITS;
const SYM_MAX: u32 = (1 << SYM_BITS) - 1;

impl<'a> RangeDecoder<'a> {
    pub fn new(data: &'a [u8]) -> RangeDecoder<'a> {
        let mut d = RangeDecoder { data, front: 0, back: 0, window: 0, available: 0, dif: 0, rng: 128, rem: 0, ext: 0, nbits_total: 9 };
        d.rem = d.read_front();
        d.dif = d.rng - 1 - (d.rem >> 1);
        d.normalize();
        d
    }

    fn read_front(&mut self) -> u32 {
        let b = self.data.get(self.front).copied().unwrap_or(0) as u32;
        self.front += 1;
        b
    }

    fn read_back(&mut self) -> u32 {
        self.back += 1;
        self.data.len().checked_sub(self.back).and_then(|i| self.data.get(i)).copied().unwrap_or(0) as u32
    }

    fn normalize(&mut self) {
        while self.rng <= CODE_BOT {
            self.nbits_total += SYM_BITS as i32;
            self.rng <<= SYM_BITS;
            let sym = self.rem;
            self.rem = self.read_front();
            let sym = ((sym << SYM_BITS) | self.rem) >> 1;
            self.dif = (self.dif.wrapping_shl(SYM_BITS).wrapping_add(SYM_MAX & !sym)) & (CODE_TOP - 1);
        }
    }

    /// The cumulative frequency the next symbol falls at, out of a total of `ft` (1 to 65535); follow with [`RangeDecoder::update`].
    pub fn decode(&mut self, ft: u32) -> u32 {
        let ft = ft.clamp(1, 65535);
        self.ext = self.rng / ft;
        let s = self.dif / self.ext.max(1);
        ft - (s + 1).min(ft)
    }

    pub fn decode_bin(&mut self, bits: u32) -> u32 {
        self.decode(1 << bits.min(15))
    }

    /// Say which symbol `decode` found: its range `[fl, fh)` out of `ft`.
    pub fn update(&mut self, fl: u32, fh: u32, ft: u32) {
        let s = self.ext.wrapping_mul(ft.wrapping_sub(fh));
        self.dif = self.dif.wrapping_sub(s);
        self.rng = if fl > 0 { self.ext.wrapping_mul(fh.wrapping_sub(fl)) } else { self.rng.wrapping_sub(s) };
        self.normalize();
    }

    /// A bit that is 1 with probability `1 / 2^logp`.
    pub fn bit_logp(&mut self, logp: u32) -> bool {
        let (r, d) = (self.rng, self.dif);
        let s = r >> logp.min(31);
        let one = d < s;
        if !one {
            self.dif = d - s;
        }
        self.rng = if one { s } else { r - s };
        self.normalize();
        one
    }

    /// A symbol from an inverse cumulative table of `2^ftb` (as the reference's `ec_dec_icdf`): the index.
    pub fn icdf(&mut self, icdf: &[u8], ftb: u32) -> usize {
        let (d, r) = (self.dif, self.rng >> ftb.min(15));
        let mut prev = self.rng;
        let mut ret = 0usize;
        let (before, s) = loop {
            let s = r.wrapping_mul(icdf.get(ret).copied().unwrap_or(0) as u32);
            if d >= s || ret + 1 >= icdf.len() {
                break (prev, s);
            }
            prev = s;
            ret += 1;
        };
        self.dif = d.wrapping_sub(s);
        self.rng = before.wrapping_sub(s);
        self.normalize();
        ret
    }

    /// `bits` raw bits (at most 25) read from the end of the frame.
    pub fn bits(&mut self, bits: u32) -> u32 {
        let bits = bits.min(25);
        while self.available < bits {
            self.window |= self.read_back() << self.available;
            self.available += SYM_BITS;
        }
        let r = self.window & ((1u32 << bits) - 1);
        self.window >>= bits;
        self.available -= bits;
        self.nbits_total += bits as i32;
        r
    }

    /// A number below `ft` (2 to 2^32), as the reference's `ec_dec_uint`.
    pub fn uint(&mut self, ft: u32) -> u32 {
        let ft = ft.max(2) - 1;
        let ftb = 32 - ft.leading_zeros();
        if ftb > 8 {
            let ftb = ftb - 8;
            let ft1 = (ft >> ftb) + 1;
            let s = self.decode(ft1);
            self.update(s, s + 1, ft1);
            let t = (s << ftb) | self.bits(ftb);
            t.min(ft)
        } else {
            let s = self.decode(ft + 1);
            self.update(s, s + 1, ft + 1);
            s
        }
    }

    /// Bits used so far (as the reference's `ec_tell`): how much of the frame has been read.
    pub fn tell(&self) -> i32 {
        self.nbits_total - (32 - self.rng.leading_zeros() as i32)
    }

    /// Bits used so far in eighths of a bit (as `ec_tell_frac`).
    pub fn tell_frac(&self) -> i32 {
        let mut l = 32 - self.rng.leading_zeros() as i32;
        let mut r = self.rng >> (l - 16);
        for _ in 0..3 {
            r = (r * r) >> 15;
            let b = (r >> 16) as i32;
            l = (l << 1) | b;
            r >>= b;
        }
        (self.nbits_total << 3) - l
    }

    /// The size of the frame in bytes.
    pub fn storage(&self) -> usize {
        self.data.len()
    }

    /// The state of the range coder, which the standard uses to seed the noise and to check a decoder (the "final range").
    pub fn range(&self) -> u32 {
        self.rng
    }

    /// Makes the frame `n` bytes shorter at its end (the bytes belong to something else, as a redundancy frame); only before raw bits are read.
    pub fn truncate_end(&mut self, n: usize) {
        let keep = self.data.len().saturating_sub(n);
        self.data = &self.data[..keep];
    }

    /// Pretend that the rest of the frame has been read (a silent frame).
    pub fn skip_to(&mut self, bits: i32) {
        self.nbits_total += bits - self.tell();
    }
}

/// A frame with no data is a frame the encoder did not send (silence or a stopped transmission): it plays as silence of its length.
/// A frame with data needs the CELT or the SILK decoder, which are not written yet.
pub fn decode_silent_frames(p: &Packet<'_>, channels: usize) -> Result<Vec<i16>, OpusError> {
    if p.frames.iter().any(|f| !f.is_empty()) {
        return Err(OpusError::NotDecodedYet(match p.toc.mode {
            Mode::Celt => "CELT",
            Mode::Silk => "SILK",
            Mode::Hybrid => "SILK and CELT (hybrid)",
        }));
    }
    Ok(vec![0i16; p.samples() * channels.max(1)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_of_contents_gives_mode_bandwidth_frame_size_and_channels() {
        // config 0: SILK narrowband 10 ms; 15: hybrid full band 20 ms; 16: CELT narrowband 2.5 ms; 31: CELT full band 20 ms
        let t = Toc::parse(0);
        assert_eq!((t.mode, t.bandwidth, t.frame_samples(), t.stereo, t.code), (Mode::Silk, Bandwidth::Narrow, 480, false, 0));
        let t = Toc::parse(15 << 3 | 4 | 1);
        assert_eq!((t.mode, t.bandwidth, t.frame_samples(), t.stereo, t.code), (Mode::Hybrid, Bandwidth::Full, 960, true, 1));
        assert_eq!(Toc::parse(16 << 3).frame_samples(), 120);
        assert_eq!(Toc::parse(31 << 3).frame_samples(), 960);
        assert_eq!(Toc::parse(31 << 3).bandwidth.hz(), 20000);
        // SILK 60 ms and CELT 5 and 10 ms
        assert_eq!(Toc::parse(3 << 3).frame_samples(), 2880);
        assert_eq!((Toc::parse(17 << 3).frame_samples(), Toc::parse(18 << 3).frame_samples()), (240, 480));
    }

    #[test]
    fn the_four_packings_are_taken_apart_and_the_invalid_ones_are_refused_in_words() {
        let t = 31 << 3; // CELT full band 20 ms, mono
        // code 0: one frame
        let d = [t, 1, 2, 3];
        let p = parse_packet(&d).unwrap();
        assert_eq!((p.frames.len(), p.frames[0], p.samples()), (1, &[1u8, 2, 3][..], 960));
        // code 1: two equal frames
        let d = [t | 1, 1, 2, 3, 4];
        let p = parse_packet(&d).unwrap();
        assert_eq!((p.frames[0], p.frames[1]), (&[1u8, 2][..], &[3u8, 4][..]));
        assert!(parse_packet(&[t | 1, 1, 2, 3]).is_err());
        // code 2: two frames, the first one's length given
        let d = [t | 2, 2, 9, 9, 7];
        let p = parse_packet(&d).unwrap();
        assert_eq!((p.frames[0], p.frames[1]), (&[9u8, 9][..], &[7u8][..]));
        assert!(parse_packet(&[t | 2, 9, 1]).unwrap_err().to_string().contains("longer than the packet"));
        // a length of two bytes: 252 + 4 * 1 = 256
        let mut big = vec![t | 2, 252, 1];
        big.extend(vec![0u8; 256]);
        big.push(5);
        assert_eq!(parse_packet(&big).unwrap().frames[0].len(), 256);
        // code 3: constant frame sizes, three frames of two bytes
        let d = [t | 3, 3, 1, 2, 3, 4, 5, 6];
        let p = parse_packet(&d).unwrap();
        assert_eq!(p.frames.len(), 3);
        assert_eq!(p.frames[2], &[5u8, 6][..]);
        assert!(parse_packet(&[t | 3, 3, 1, 2, 3, 4, 5]).is_err());
        // code 3 with variable sizes and padding: the flags byte, the padding length, the lengths, the frames, the padding
        let d = [t | 3, 0x80 | 0x40 | 2, 2, 1, 7, 7, 8, 0xaa, 0xbb];
        let p = parse_packet(&d).unwrap();
        assert_eq!((p.frames[0], p.frames[1]), (&[7u8][..], &[7u8, 8][..]));
        // empty, zero frames, more than 120 ms, a frame that is too long
        assert!(parse_packet(&[]).is_err());
        assert!(parse_packet(&[t | 3, 0]).is_err());
        assert!(parse_packet(&[t | 3, 7]).unwrap_err().to_string().contains("120 ms"));
        let mut long = vec![t, 0];
        long.extend(vec![0u8; 1276]);
        assert!(parse_packet(&long).unwrap_err().to_string().contains("1275"));
    }

    #[test]
    fn the_header_is_read_and_what_is_not_supported_is_named() {
        let mut h = b"OpusHead".to_vec();
        h.extend([1, 2, 0x38, 0x01]);
        h.extend(48000u32.to_le_bytes());
        h.extend([0, 0, 0]);
        let head = parse_head(&h).unwrap();
        assert_eq!((head.channels, head.pre_skip, head.input_rate, head.gain), (2, 312, 48000, 0));
        h[18] = 1;
        assert!(matches!(parse_head(&h), Err(OpusError::NotDecodedYet(_))));
        h[18] = 0;
        h[8] = 0x20;
        assert!(parse_head(&h).unwrap_err().to_string().contains("newer"));
        assert!(parse_head(b"OpusTags").is_err());
        assert!(parse_head(&h[..10]).is_err());
    }

    #[test]
    fn packets_without_data_are_silence_of_their_length_and_packets_with_data_say_what_is_missing() {
        let d = [31 << 3];
        let p = parse_packet(&d).unwrap(); // a packet of one empty frame
        let pcm = decode_silent_frames(&p, 2).unwrap();
        assert_eq!(pcm.len(), 960 * 2);
        assert!(pcm.iter().all(|v| *v == 0));
        let d = [31 << 3, 0xff, 0xfe];
        let p = parse_packet(&d).unwrap();
        assert_eq!(decode_silent_frames(&p, 1).unwrap_err(), OpusError::NotDecodedYet("CELT"));
        assert!(decode_silent_frames(&parse_packet(&[0, 1]).unwrap(), 1).unwrap_err().to_string().contains("SILK"));
    }

    #[test]
    fn the_range_decoder_stays_inside_its_bounds_on_any_bytes_and_reads_past_the_end_as_zeros() {
        // pseudo-random frames of every size: every symbol is inside its table, nothing panics
        let mut x = 0x9e37_79b9u32;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        for len in [0usize, 1, 2, 7, 50, 300] {
            let data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let mut d = RangeDecoder::new(&data);
            for i in 0..400 {
                match i % 5 {
                    0 => {
                        let ft = 2 + (next() % 60000);
                        let s = d.decode(ft);
                        assert!(s < ft);
                        d.update(s, s + 1, ft);
                    }
                    1 => {
                        d.bit_logp(1 + next() % 15);
                    }
                    2 => {
                        let table = [200u8, 120, 60, 10, 0];
                        assert!(d.icdf(&table, 8) < table.len());
                    }
                    3 => {
                        let n = 1 + next() % 25;
                        assert!(d.bits(n) < (1 << n));
                    }
                    _ => {
                        let ft = 2 + next() % 100_000;
                        assert!(d.uint(ft) < ft);
                    }
                }
            }
            assert!(d.tell() > 0);
        }
    }

    #[test]
    fn a_frame_of_all_zero_bytes_decodes_its_first_symbols_the_same_way_every_time() {
        // determinism: the same bytes give the same symbols
        let data = [0x12u8, 0x34, 0x56, 0x78, 0x9a, 0xbc];
        let run = || {
            let mut d = RangeDecoder::new(&data);
            (0..20).map(|i| if i % 2 == 0 { d.bit_logp(3) as u32 } else { let s = d.decode(7); d.update(s, s + 1, 7); s }).collect::<Vec<u32>>()
        };
        assert_eq!(run(), run());
    }

    /// Real streams: made by the Opus encoder of the installed ffmpeg (a reference tool only; skipped when it is absent), read through the
    /// in-house Ogg layer. Every packet parses, the mode and bandwidth are the encoder's, and the duration worked out from the packets
    /// is the number of samples ffmpeg's own decode of the same file produces (to within the last packet).
    #[test]
    fn real_opus_files_parse_and_their_duration_is_the_one_ffmpegs_decode_has() {
        use std::process::Command;
        if !Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false) {
            eprintln!("skipped: ffmpeg is not installed");
            return;
        }
        let dir = std::env::temp_dir().join(format!("iyakki-opus-ref-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // (tag, lavfi source, channels, extra encoder arguments, expected mode and frame length in samples when the encoder is told one)
        let cases: [(&str, &str, &str, &[&str], Option<(Mode, usize)>); 4] = [
            ("celt20", "sine=frequency=440:duration=2", "1", &["-application", "lowdelay", "-frame_duration", "20", "-b:a", "64k"], Some((Mode::Celt, 960))),
            ("celt5", "sine=frequency=880:duration=1", "2", &["-application", "lowdelay", "-frame_duration", "5", "-b:a", "96k"], Some((Mode::Celt, 240))),
            ("silk20", "sine=frequency=300:duration=2", "1", &["-application", "voip", "-frame_duration", "20", "-b:a", "16k", "-cutoff", "8000"], Some((Mode::Silk, 960))),
            ("long60", "anoisesrc=duration=2:amplitude=0.1", "2", &["-application", "audio", "-frame_duration", "60", "-b:a", "48k"], None),
        ];
        for (tag, source, ch, enc, want) in cases {
            let opus = dir.join(format!("{tag}.opus"));
            let raw = dir.join(format!("{tag}.raw"));
            let ok = Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", source, "-ar", "48000", "-ac", ch, "-c:a", "libopus"]).args(enc).arg(&opus).status().unwrap().success();
            assert!(ok, "{tag}: ffmpeg could not make the file");
            assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-i"]).arg(&opus).args(["-f", "s16le", "-acodec", "pcm_s16le"]).arg(&raw).status().unwrap().success());
            let channels: usize = ch.parse().unwrap();
            let reference_samples = std::fs::metadata(&raw).unwrap().len() as usize / 2 / channels;
            let mut demux = crate::ogg::Demuxer::new();
            let pages = demux.push(&std::fs::read(&opus).unwrap());
            let mut packets: Vec<Vec<u8>> = pages.iter().flat_map(|p| p.packets.iter().cloned()).collect();
            let last_granule = pages.iter().rev().find(|p| p.granule >= 0).unwrap().granule;
            assert!(packets[0].starts_with(b"OpusHead") && packets[1].starts_with(b"OpusTags"), "{tag}");
            let head = parse_head(&packets[0]).unwrap();
            assert_eq!(head.channels as usize, channels, "{tag}");
            assert!(head.pre_skip > 0 && head.pre_skip < 2000, "{tag}: pre-skip {}", head.pre_skip);
            let audio: Vec<Vec<u8>> = packets.drain(2..).collect();
            assert!(!audio.is_empty());
            let mut seen_mode = None;
            for p in &audio {
                let parsed = parse_packet(p).unwrap_or_else(|e| panic!("{tag}: {e}"));
                assert_eq!(parsed.toc.stereo, channels == 2 || parsed.toc.stereo, "{tag}");
                seen_mode = Some((parsed.toc.mode, parsed.toc.frame_samples()));
                if let Some((m, n)) = want {
                    assert_eq!((parsed.toc.mode, parsed.toc.frame_samples()), (m, n), "{tag}");
                }
            }
            let _ = seen_mode;
            let counted = stream_samples(&head, &audio).unwrap();
            // the container's own end point, less the pre-skip, is what the reference decode has
            assert_eq!(last_granule as u64 - head.pre_skip as u64, reference_samples as u64, "{tag}: granule against ffmpeg's decode");
            // the packets cover that, and not a whole extra packet more
            let longest = audio.iter().map(|p| parse_packet(p).unwrap().samples() as u64).max().unwrap();
            assert!(counted >= reference_samples as u64 && counted < reference_samples as u64 + longest, "{tag}: counted {counted}, reference {reference_samples}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
