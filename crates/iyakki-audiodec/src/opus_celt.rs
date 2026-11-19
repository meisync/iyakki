//! The CELT decoder of Opus (RFC 6716 section 4.3), written in-house for the 48 kHz mode that every Opus stream uses: the four frame
//! sizes (2.5, 5, 10 and 20 ms), mono and stereo, in 32-bit floating point.
//!
//! One frame is decoded in this order, every step reading from the range decoder of the frame: the silence flag, the post-filter
//! parameters, the transient flag, the coarse band energies, the time and frequency resolution changes per band, the spreading
//! decision, the allocation boosts and trim, the bit allocation (which also reads the band skips, the intensity band and the dual
//! stereo flag), the fine energies, the band shapes (pyramid vector quantisation, split in halves or in mid and side as the bits
//! allow, folded from lower bands where a band has no bits), the anti-collapse flag and the leftover energy bits. The sound is then
//! made from the band energies and shapes: inverse MDCT with the low-overlap window, the pitch post-filter (a comb filter) and the
//! de-emphasis filter.
//!
//! The constant tables are in `opus_tables.rs` (data from the standard's reference decoder, BSD licence, see legal/); everything
//! else here is this project's own code. Nothing in it can panic on hostile bytes: reads past the end of a frame give zeros as the
//! standard says, and every index is bounded by sizes this mode fixes.

use crate::opus::RangeDecoder;
use crate::opus_tables::{BAND_ALLOCATION, CACHE_BITS, CACHE_CAPS, CACHE_INDEX, EBANDS, E_PROB_MODEL, LOG_N};

/// A complex fast Fourier transform for sizes whose prime factors are 2, 3 and 5 (the Opus frame sizes are 15 times a power of two),
/// by recursive decimation in time. `tw[j] = exp(-2 pi i j / n)`.
struct Fft {
    n: usize,
    tw: Vec<(f64, f64)>,
}

impl Fft {
    fn new(n: usize) -> Fft {
        let tw = (0..n).map(|j| {
            let a = -2.0 * std::f64::consts::PI * j as f64 / n as f64;
            (a.cos(), a.sin())
        }).collect();
        Fft { n, tw }
    }

    fn rec(&self, n: usize, inp: &[(f64, f64)], stride: usize, out: &mut [(f64, f64)]) {
        if n == 1 {
            out[0] = inp[0];
            return;
        }
        let p = [2usize, 3, 5].into_iter().find(|p| n % p == 0).unwrap_or(n);
        let m = n / p;
        for r in 0..p {
            let (a, b) = (r * m, (r + 1) * m);
            self.rec(m, &inp[r * stride..], stride * p, &mut out[a..b]);
        }
        let step = self.n / n;
        let mut z = [(0.0f64, 0.0f64); 5];
        for k in 0..m {
            for r in 0..p {
                z[r] = out[r * m + k];
            }
            for q in 0..p {
                let (mut sr, mut si) = (z[0].0, z[0].1);
                for r in 1..p {
                    let (wr, wi) = self.tw[(r * (k + q * m) * step) % self.n];
                    sr += z[r].0 * wr - z[r].1 * wi;
                    si += z[r].0 * wi + z[r].1 * wr;
                }
                out[k + q * m] = (sr, si);
            }
        }
    }

    fn run(&self, data: &mut [(f64, f64)]) {
        let tmp = data.to_vec();
        self.rec(self.n, &tmp, 1, data);
    }
}

/// The inverse MDCT of `m` coefficients to `2 m` samples (`x[j] = sum_k X[k] cos(pi/m (j + 1/2 + m/2)(k + 1/2))`) through a DCT-IV that
/// a complex transform of half the size computes.
struct Imdct {
    m: usize,
    fft: Fft,
    /// `exp(-i pi (4k + 1) / (4 m))` before the transform and `exp(-i pi k / m)` after it
    pre: Vec<(f64, f64)>,
    post: Vec<(f64, f64)>,
}

impl Imdct {
    fn new(n: usize) -> Imdct {
        let m = n / 2;
        let h = m / 2;
        let pre = (0..h).map(|k| {
            let a = -std::f64::consts::PI * (4 * k + 1) as f64 / (4 * m) as f64;
            (a.cos(), a.sin())
        }).collect();
        let post = (0..h).map(|k| {
            let a = -std::f64::consts::PI * k as f64 / m as f64;
            (a.cos(), a.sin())
        }).collect();
        Imdct { m, fft: Fft::new(h), pre, post }
    }

    /// `y[j] = sum_k x[k] cos(pi / m (j + 1/2)(k + 1/2))`
    fn dct4(&self, x: &[f32], y: &mut [f32]) {
        let m = self.m;
        let h = m / 2;
        let mut z: Vec<(f64, f64)> = (0..h).map(|k| {
            let (a, b) = (x[2 * k] as f64, x[m - 1 - 2 * k] as f64);
            let (c, s) = self.pre[k];
            (a * c - b * s, a * s + b * c)
        }).collect();
        self.fft.run(&mut z);
        for k in 0..h {
            let (c, s) = self.post[k];
            let (ur, ui) = (z[k].0 * c - z[k].1 * s, z[k].0 * s + z[k].1 * c);
            y[2 * k] = ur as f32;
            y[m - 1 - 2 * k] = -ui as f32;
        }
    }

    fn run(&self, coef: &[f32], out: &mut [f32]) {
        let m = self.m;
        let mut y = vec![0.0f32; m];
        self.dct4(coef, &mut y);
        for (j, o) in out.iter_mut().enumerate().take(2 * m) {
            *o = if j < m / 2 {
                y[j + m / 2]
            } else if j < 3 * m / 2 {
                -y[3 * m / 2 - 1 - j]
            } else {
                -y[j - 3 * m / 2]
            };
        }
    }

    /// The definition summed directly (slow), for the test.
    #[cfg(test)]
    fn direct(coef: &[f32]) -> Vec<f64> {
        let m = coef.len();
        (0..2 * m).map(|j| coef.iter().enumerate().map(|(k, &c)| c as f64 * (std::f64::consts::PI / m as f64 * (j as f64 + 0.5 + m as f64 / 2.0) * (k as f64 + 0.5)).cos()).sum()).collect()
    }
}

const NB_BANDS: usize = 21;
const SHORT: usize = 120;
const OVERLAP: usize = 120;
const BITRES: i32 = 3;
const MAX_FINE_BITS: i32 = 8;
const FINE_OFFSET: i32 = 21;
const QTHETA_OFFSET: i32 = 4;
const QTHETA_OFFSET_TWOPHASE: i32 = 16;
/// Samples of history kept for the post-filter (the longest pitch period, 1022, plus the taps).
const HISTORY: usize = 1280;
const MIN_PERIOD: i32 = 15;

const SPREAD_NONE: i32 = 0;
const SPREAD_AGGRESSIVE: i32 = 3;

const TRIM_ICDF: [u8; 11] = [126, 124, 119, 109, 87, 41, 19, 9, 4, 2, 0];
const SPREAD_ICDF: [u8; 4] = [25, 23, 2, 0];
const TAPSET_ICDF: [u8; 3] = [2, 1, 0];
const SMALL_ENERGY_ICDF: [u8; 3] = [2, 1, 0];
const LOG2_FRAC: [u8; 24] = [0, 8, 13, 16, 19, 21, 23, 24, 26, 27, 28, 29, 30, 31, 32, 32, 33, 34, 34, 35, 36, 36, 37, 37];
const TF_SELECT: [[i8; 8]; 4] = [[0, -1, 0, -1, 0, -1, 0, -1], [0, -1, 0, -2, 1, 0, 1, -1], [0, -2, 0, -3, 2, 0, 1, -1], [0, -2, 0, -3, 3, 0, 1, -1]];
const COMB_GAINS: [[f32; 3]; 3] = [[0.306_640_62, 0.217_041_02, 0.129_638_67], [0.463_867_2, 0.268_066_4, 0.0], [0.799_804_7, 0.100_097_66, 0.0]];
/// The mean energy of each band (log2 units), which the coarse energy is relative to.
const E_MEANS: [f32; NB_BANDS] = [6.4375, 6.25, 5.75, 5.3125, 5.0625, 4.8125, 4.5, 4.375, 4.875, 4.6875, 4.5625, 4.4375, 4.875, 4.625, 4.3125, 4.5, 4.375, 4.625, 4.75, 4.4375, 3.75];
const PRED_COEF: [f32; 4] = [29440.0 / 32768.0, 26112.0 / 32768.0, 21248.0 / 32768.0, 16384.0 / 32768.0];
const BETA_COEF: [f32; 4] = [30147.0 / 32768.0, 22282.0 / 32768.0, 12124.0 / 32768.0, 6554.0 / 32768.0];
const BETA_INTRA: f32 = 4915.0 / 32768.0;
/// Index order that takes the Hadamard-ordered blocks back to time order, for 2, 4, 8 and 16 blocks.
const ORDERY: [usize; 30] = [1, 0, 3, 0, 2, 1, 7, 0, 4, 3, 6, 1, 5, 2, 15, 0, 8, 7, 12, 3, 11, 4, 14, 1, 9, 6, 13, 2, 10, 5];

fn band_width(i: usize) -> i32 {
    (EBANDS[i + 1] - EBANDS[i]) as i32
}

fn cache_row(lm_plus: usize, band: usize) -> &'static [u8] {
    let at = CACHE_INDEX[lm_plus * NB_BANDS + band];
    if at < 0 {
        return &[];
    }
    &CACHE_BITS[at as usize..]
}

/// The number of pulses for the index a cache holds (the cache is indexed by a pseudo pulse count that grows geometrically).
fn pulses_of(i: i32) -> i32 {
    if i < 8 {
        i
    } else {
        (8 + (i & 7)) << ((i >> 3) - 1)
    }
}

/// The pseudo pulse count that costs closest to `bits` (in eighths) in the given band at the given size.
fn bits_to_pulses(band: usize, lm: i32, bits: i32) -> i32 {
    let cache = cache_row((lm + 1) as usize, band);
    if cache.is_empty() {
        return 0;
    }
    let (mut lo, mut hi) = (0i32, cache[0] as i32);
    let bits = bits - 1;
    for _ in 0..6 {
        let mid = (lo + hi + 1) >> 1;
        if cache.get(mid as usize).copied().unwrap_or(255) as i32 >= bits {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let low_cost = if lo == 0 { -1 } else { cache[lo as usize] as i32 };
    if bits - low_cost <= cache[hi as usize] as i32 - bits {
        lo
    } else {
        hi
    }
}

fn pulses_to_bits(band: usize, lm: i32, pulses: i32) -> i32 {
    if pulses == 0 {
        return 0;
    }
    let cache = cache_row((lm + 1) as usize, band);
    cache.get(pulses as usize).map(|&b| b as i32 + 1).unwrap_or(0)
}

fn lcg(seed: u32) -> u32 {
    seed.wrapping_mul(1664525).wrapping_add(1013904223)
}

fn ilog(v: u32) -> i32 {
    32 - v.leading_zeros() as i32
}

fn isqrt(v: u32) -> u32 {
    let mut r = (v as f64).sqrt() as u32;
    while (r as u64) * (r as u64) > v as u64 {
        r -= 1;
    }
    while ((r + 1) as u64) * ((r + 1) as u64) <= v as u64 {
        r += 1;
    }
    r
}

fn frac_mul16(a: i32, b: i32) -> i32 {
    (16384 + (a as i16 as i32) * (b as i16 as i32)) >> 15
}

/// A cosine that gives the same integers on every machine: the split angle decides the bit allocation, so it must not differ.
fn bitexact_cos(x: i32) -> i32 {
    let tmp = (4096 + x * x) >> 13;
    let x2 = tmp;
    let x2 = (32767 - x2) + frac_mul16(x2, -7651 + frac_mul16(x2, 8277 + frac_mul16(-626, x2)));
    1 + x2
}

fn bitexact_log2tan(isin: i32, icos: i32) -> i32 {
    let lc = ilog(icos as u32);
    let ls = ilog(isin as u32);
    let icos = icos << (15 - lc);
    let isin = isin << (15 - ls);
    (ls - lc) * (1 << 11) + frac_mul16(isin, frac_mul16(isin, -2597) + 7932) - frac_mul16(icos, frac_mul16(icos, -2597) + 7932)
}

/// What one decoded frame's bit allocation says.
struct Allocation {
    pulses: [i32; NB_BANDS],
    fine_bits: [i32; NB_BANDS],
    fine_priority: [i32; NB_BANDS],
    intensity: i32,
    dual_stereo: bool,
    balance: i32,
    coded_bands: i32,
}

/// The decoder state that lives from frame to frame.
pub struct CeltDecoder {
    /// Output channels (1 or 2).
    channels: usize,
    imdct: Vec<Imdct>,
    window: [f32; OVERLAP],
    /// The tail of each channel's previous frame, to be added to the start of the next.
    overlap_tail: [[f32; OVERLAP]; 2],
    /// The last samples of each channel after the post-filter (before de-emphasis).
    history: [Vec<f32>; 2],
    deemph: [f32; 2],
    old_band_e: [[f32; NB_BANDS]; 2],
    old_log_e: [[f32; NB_BANDS]; 2],
    old_log_e2: [[f32; NB_BANDS]; 2],
    rng: u32,
    period: i32,
    period_old: i32,
    gain: f32,
    gain_old: f32,
    tapset: usize,
    tapset_old: usize,
    /// Frames concealed in a row (0 after a good frame).
    loss_count: u32,
    last_pitch: usize,
    /// A slowly rising floor under the band energies, which long losses fade to.
    background: [[f32; NB_BANDS]; 2],
}

/// What the packet's table of contents tells the frame decoder.
#[derive(Clone, Copy, Debug)]
pub struct FrameParams {
    /// The channels the frame was coded with (1 or 2).
    pub stream_channels: usize,
    /// The number of bands coded (13, 17, 19 or 21 for narrow, wide, super wide and full band).
    pub end_band: usize,
    /// 0 to 3 for frames of 2.5, 5, 10 and 20 ms.
    pub lm: usize,
    /// The first band coded: 0, or 17 in hybrid mode (the speech layer carries the bands below).
    pub start: usize,
}

impl CeltDecoder {
    pub fn new(channels: usize) -> CeltDecoder {
        let w = std::array::from_fn(|i| {
            let s = (0.5 * std::f64::consts::PI * (i as f64 + 0.5) / OVERLAP as f64).sin();
            (0.5 * std::f64::consts::PI * s * s).sin() as f32
        });
        CeltDecoder {
            channels: channels.clamp(1, 2),
            imdct: (0..4).map(|lm| Imdct::new(2 * (SHORT << lm))).collect(),
            window: w,
            overlap_tail: [[0.0; OVERLAP]; 2],
            history: [vec![0.0; HISTORY], vec![0.0; HISTORY]],
            deemph: [0.0; 2],
            old_band_e: [[0.0; NB_BANDS]; 2],
            old_log_e: [[-28.0; NB_BANDS]; 2],
            old_log_e2: [[-28.0; NB_BANDS]; 2],
            rng: 0,
            period: 0,
            period_old: 0,
            gain: 0.0,
            gain_old: 0.0,
            tapset: 0,
            tapset_old: 0,
            loss_count: 0,
            last_pitch: 0,
            background: [[0.0; NB_BANDS]; 2],
        }
    }

    /// Forgets everything heard (a change of mode in the stream).
    pub fn reset(&mut self) {
        *self = CeltDecoder::new(self.channels);
    }

    /// Output channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Decodes one frame (`data` has at least two bytes). Returns the sound at 48 kHz, interleaved, full scale at plus and minus one.
    pub fn decode(&mut self, data: &[u8], p: FrameParams) -> Vec<f32> {
        let mut dec = RangeDecoder::new(data);
        self.decode_frame(&mut dec, p)
    }

    /// As [`CeltDecoder::decode`], reading from a range decoder that may already have been used (hybrid mode: the speech layer comes first).
    pub fn decode_frame(&mut self, dec: &mut RangeDecoder<'_>, p: FrameParams) -> Vec<f32> {
        let lm = p.lm.min(3);
        let m = 1usize << lm;
        let n = SHORT * m;
        let c = p.stream_channels.clamp(1, 2);
        let cc = self.channels;
        let end = p.end_band.clamp(1, NB_BANDS);
        let start = p.start.min(NB_BANDS - 1);
        let len = dec.storage() as i32;
        let mut total_bits = len * 8;
        let mut tell = dec.tell();
        let silence = if tell >= total_bits {
            true
        } else if tell == 1 {
            dec.bit_logp(15)
        } else {
            false
        };
        if silence {
            tell = len * 8;
            dec.skip_to(tell);
        }
        let mut post_gain = 0.0f32;
        let mut post_pitch = 0i32;
        let mut post_tapset = 0usize;
        if start == 0 && tell + 16 <= total_bits {
            if dec.bit_logp(1) {
                let octave = dec.uint(6);
                post_pitch = (16 << octave) + dec.bits(4 + octave) as i32 - 1;
                let qg = dec.bits(3);
                if dec.tell() + 2 <= total_bits {
                    post_tapset = dec.icdf(&TAPSET_ICDF, 2);
                }
                post_gain = 0.09375 * (qg as f32 + 1.0);
            }
            tell = dec.tell();
        }
        let transient = if lm > 0 && tell + 3 <= total_bits {
            let t = dec.bit_logp(3);
            tell = dec.tell();
            t
        } else {
            false
        };
        let short_blocks = if transient { m } else { 0 };
        let intra = if tell + 3 <= total_bits { dec.bit_logp(3) } else { false };
        // the coarse energies; a mono frame in a decoder that last heard stereo starts from the louder of the two
        if c == 1 {
            for i in 0..NB_BANDS {
                self.old_band_e[0][i] = self.old_band_e[0][i].max(self.old_band_e[1][i]);
            }
        }
        unquant_coarse_energy(&mut *dec, start, end, &mut self.old_band_e, intra, c, lm);
        let tf_res = tf_decode(&mut *dec, start, end, transient, lm);
        tell = dec.tell();
        let spread = if tell + 4 <= total_bits { dec.icdf(&SPREAD_ICDF, 5) as i32 } else { 2 };
        // the allocation boosts of single bands
        let mut cap = [0i32; NB_BANDS];
        for (i, cp) in cap.iter_mut().enumerate() {
            let nn = band_width(i) << lm;
            *cp = (CACHE_CAPS[NB_BANDS * (2 * lm + c - 1) + i] as i32 + 64) * c as i32 * nn >> 2;
        }
        let mut offsets = [0i32; NB_BANDS];
        let mut dynalloc_logp = 6u32;
        total_bits <<= BITRES;
        let mut tell_f = dec.tell_frac();
        for i in start..end {
            let width = (c as i32 * band_width(i)) << lm;
            let quanta = (width << BITRES).min((6 << BITRES).max(width));
            let mut loop_logp = dynalloc_logp;
            let mut boost = 0;
            while tell_f + ((loop_logp as i32) << BITRES) < total_bits && boost < cap[i] {
                let flag = dec.bit_logp(loop_logp);
                tell_f = dec.tell_frac();
                if !flag {
                    break;
                }
                boost += quanta;
                total_bits -= quanta;
                loop_logp = 1;
            }
            offsets[i] = boost;
            if boost > 0 {
                dynalloc_logp = 2.max(dynalloc_logp as i32 - 1) as u32;
            }
        }
        let alloc_trim = if tell_f + (6 << BITRES) <= total_bits { dec.icdf(&TRIM_ICDF, 7) as i32 } else { 5 };
        let mut bits = ((len * 8) << BITRES) - dec.tell_frac() - 1;
        let anti_collapse_rsv = if transient && lm >= 2 && bits >= ((lm as i32 + 2) << BITRES) { 1 << BITRES } else { 0 };
        bits -= anti_collapse_rsv;
        let al = compute_allocation(&mut *dec, start, end, &offsets, &cap, alloc_trim, bits, c, lm as i32);
        // fine energy
        for i in start..end {
            if al.fine_bits[i] <= 0 {
                continue;
            }
            for ch in 0..c {
                let q2 = dec.bits(al.fine_bits[i] as u32) as f32;
                let off = (q2 + 0.5) * (1u32 << (14 - al.fine_bits[i])) as f32 * (1.0 / 16384.0) - 0.5;
                self.old_band_e[ch][i] += off;
            }
        }
        // the band shapes
        let mut x = vec![0.0f32; c * n];
        let mut collapse = [[0u8; 2]; NB_BANDS];
        {
            let (xa, xb) = x.split_at_mut(n);
            let y = if c == 2 { Some(xb) } else { None };
            let mut bands = BandDecoder { dec: &mut *dec, lm: lm as i32, seed: self.rng, spread, remaining: 0 };
            bands.all_bands(start, end, xa, y, &mut collapse, &al, short_blocks != 0, &tf_res, (len * (8 << BITRES)) - anti_collapse_rsv);
            self.rng = bands.seed;
        }
        let anti_collapse_on = anti_collapse_rsv > 0 && dec.bits(1) != 0;
        // leftover bits refine the energies
        let mut bits_left = len * 8 - dec.tell();
        for prio in 0..2 {
            let mut i = start;
            while i < end && bits_left >= c as i32 {
                if al.fine_bits[i] < MAX_FINE_BITS && al.fine_priority[i] == prio {
                    for ch in 0..c {
                        let q2 = dec.bits(1) as f32;
                        let off = (q2 - 0.5) * (1u32 << (14 - al.fine_bits[i] - 1)) as f32 * (1.0 / 16384.0);
                        self.old_band_e[ch][i] += off;
                        bits_left -= 1;
                    }
                }
                i += 1;
            }
        }
        if anti_collapse_on {
            self.anti_collapse(&mut x, &collapse, lm, c, n, start, end, &al.pulses);
        }
        // band energies as amplitudes
        let mut band_e = [[0.0f32; NB_BANDS]; 2];
        for ch in 0..c {
            for i in start..end {
                band_e[ch][i] = (self.old_band_e[ch][i] + E_MEANS[i]).exp2();
            }
        }
        if silence {
            band_e = [[0.0; NB_BANDS]; 2];
            self.old_band_e = [[-28.0; NB_BANDS]; 2];
        }
        // synthesis: the spectrum, the inverse transform, the post-filter, the de-emphasis
        let mut freq = vec![0.0f32; 2 * n];
        for ch in 0..c {
            for i in start..end {
                let (lo, hi) = (m * EBANDS[i] as usize, m * EBANDS[i + 1] as usize);
                for j in lo..hi {
                    freq[ch * n + j] = x[ch * n + j] * band_e[ch][i];
                }
            }
        }
        if cc == 2 && c == 1 {
            let (a, b) = freq.split_at_mut(n);
            b.copy_from_slice(a);
        }
        if cc == 1 && c == 2 {
            for j in 0..n {
                freq[j] = 0.5 * (freq[j] + freq[n + j]);
            }
        }
        let mut out = vec![0.0f32; n * cc];
        for ch in 0..cc {
            let mut sig = vec![0.0f32; HISTORY + n];
            sig[..HISTORY].copy_from_slice(&self.history[ch]);
            self.synthesise(&freq[ch * n..(ch + 1) * n], lm, short_blocks, ch, &mut sig[HISTORY..]);
            self.period = self.period.max(MIN_PERIOD);
            self.period_old = self.period_old.max(MIN_PERIOD);
            comb_filter(&mut sig, HISTORY, SHORT, self.period_old, self.period, self.gain_old, self.gain, self.tapset_old, self.tapset, &self.window);
            if lm != 0 {
                comb_filter(&mut sig, HISTORY + SHORT, n - SHORT, self.period, post_pitch, self.gain, post_gain, self.tapset, post_tapset, &self.window);
            }
            self.history[ch].copy_from_slice(&sig[n..n + HISTORY]);
            // de-emphasis: y[i] = x[i] + 0.85 y[i-1]
            let mut mem = self.deemph[ch];
            for j in 0..n {
                let v = sig[HISTORY + j] + mem;
                mem = 0.850_006_1 * v;
                out[j * cc + ch] = v * (1.0 / 32768.0);
            }
            self.deemph[ch] = mem;
        }
        self.period_old = self.period;
        self.gain_old = self.gain;
        self.tapset_old = self.tapset;
        self.period = post_pitch;
        self.gain = post_gain;
        self.tapset = post_tapset;
        if lm != 0 {
            self.period_old = self.period;
            self.gain_old = self.gain;
            self.tapset_old = self.tapset;
        }
        if c == 1 {
            self.old_band_e[1] = self.old_band_e[0];
        }
        // remember the energies of this and the two frames before
        if !transient {
            self.old_log_e2 = self.old_log_e;
            self.old_log_e = self.old_band_e;
            for ch in 0..2 {
                for i in 0..NB_BANDS {
                    self.background[ch][i] = (self.background[ch][i] + m as f32 * 0.001).min(self.old_band_e[ch][i]);
                }
            }
        } else {
            for ch in 0..2 {
                for i in 0..NB_BANDS {
                    self.old_log_e[ch][i] = self.old_log_e[ch][i].min(self.old_band_e[ch][i]);
                }
            }
        }
        for ch in 0..2 {
            for i in (0..start).chain(end..NB_BANDS) {
                self.old_band_e[ch][i] = 0.0;
                self.old_log_e[ch][i] = -28.0;
                self.old_log_e2[ch][i] = -28.0;
            }
        }
        self.rng = dec.range();
        self.loss_count = 0;
        out
    }

    /// Keeps `samples` (after the post-filter) as history and runs the de-emphasis into `out` (interleaved, `cc` channels).
    fn finish_channel(&mut self, ch: usize, samples: &[f32], out: &mut [f32], cc: usize) {
        let n = samples.len();
        let mut all = Vec::with_capacity(HISTORY + n);
        all.extend_from_slice(&self.history[ch]);
        all.extend_from_slice(samples);
        self.history[ch].copy_from_slice(&all[n..n + HISTORY]);
        let mut mem = self.deemph[ch];
        for (j, &x) in samples.iter().enumerate() {
            let v = x + mem;
            mem = 0.850_006_1 * v;
            out[j * cc + ch] = v * (1.0 / 32768.0);
        }
        self.deemph[ch] = mem;
    }

    /// Makes one frame of sound for a frame that did not arrive (section 4.3.6 of the standard leaves the method to the decoder):
    /// the first losses repeat the last pitch cycle through the LPC synthesis filter of the recent sound with decaying gain; longer
    /// losses (and hybrid frames, whose lower bands are the speech layer's) play noise at the decaying band energies.
    pub fn decode_lost(&mut self, lm: usize, start: usize, end: usize) -> Vec<f32> {
        let lm = lm.min(3);
        let m = 1usize << lm;
        let n = SHORT * m;
        let cc = self.channels;
        let end = end.clamp(1, NB_BANDS);
        let start = start.min(end - 1);
        let mut out = vec![0.0f32; n * cc];
        if self.loss_count >= 5 || start != 0 {
            let mut band_e = [[0.0f32; NB_BANDS]; 2];
            if self.loss_count >= 5 {
                for ch in 0..cc {
                    for i in start..end {
                        band_e[ch][i] = (self.background[ch][i] + E_MEANS[i]).exp2();
                    }
                }
            } else {
                let decay = if self.loss_count == 0 { 1.5 } else { 0.5 };
                for ch in 0..cc {
                    for i in start..end {
                        self.old_band_e[ch][i] -= decay;
                        band_e[ch][i] = (self.old_band_e[ch][i] + E_MEANS[i]).exp2();
                    }
                }
            }
            let mut seed = self.rng;
            for ch in 0..cc {
                let mut freq = vec![0.0f32; n];
                for i in start..end {
                    let (lo, hi) = (m * EBANDS[i] as usize, m * EBANDS[i + 1] as usize);
                    let band = &mut freq[lo..hi];
                    for v in band.iter_mut() {
                        seed = lcg(seed);
                        *v = ((seed as i32) >> 20) as f32;
                    }
                    renormalise(band, band_e[ch][i]);
                }
                let mut sig = vec![0.0f32; n];
                self.synthesise(&freq, lm, 0, ch, &mut sig);
                self.finish_channel(ch, &sig, &mut out, cc);
            }
            self.rng = seed;
        } else {
            const LPC_ORDER: usize = 24;
            const HIST: usize = 1024;
            let first = self.loss_count == 0;
            if first {
                // the pitch period: the lag (100 to 720 samples) at which the last 480 samples best repeat
                let h = &self.history[0];
                let base = HISTORY - HIST;
                let win = 400usize;
                let mut best = (0.0f64, 100usize);
                for lag in 100..=720usize {
                    let (mut xy, mut yy) = (0.0f64, 1e-9f64);
                    let mut i = 0;
                    while i < win {
                        let a = h[HISTORY - win + i] as f64;
                        let b = h[HISTORY - win + i - lag] as f64;
                        xy += a * b;
                        yy += b * b;
                        i += 2;
                    }
                    let score = if xy > 0.0 { xy * xy / yy } else { 0.0 };
                    if score > best.0 {
                        best = (score, lag);
                    }
                }
                let _ = base;
                self.last_pitch = best.1;
            }
            let pitch = self.last_pitch.clamp(100, 720);
            let fade = if first { 1.0f32 } else { 0.8 };
            let len = n + OVERLAP;
            for ch in 0..cc {
                let hist: Vec<f32> = self.history[ch][HISTORY - HIST..].to_vec();
                // LPC of the recent sound
                let mut ac = [0.0f64; LPC_ORDER + 1];
                for (k, a) in ac.iter_mut().enumerate() {
                    *a = (k..HIST).map(|i| hist[i] as f64 * hist[i - k] as f64).sum();
                }
                ac[0] *= 1.0001;
                for (k, a) in ac.iter_mut().enumerate().skip(1) {
                    let w = 0.008 * k as f64;
                    *a -= *a * w * w;
                }
                let lpc = levinson(&ac, LPC_ORDER);
                // the whitened history is the excitation to repeat
                let mut exc = vec![0.0f32; HIST];
                for i in 0..HIST {
                    let mut v = hist[i] as f64;
                    for (k, &a) in lpc.iter().enumerate() {
                        if i > k {
                            v += a * hist[i - 1 - k] as f64;
                        }
                    }
                    exc[i] = v as f32;
                }
                let period = pitch.min(HIST / 2);
                let (mut e1, mut e2) = (1.0f64, 1.0f64);
                for i in 0..period {
                    e1 += (exc[HIST - period + i] as f64).powi(2) / 256.0;
                    e2 += (exc[HIST - 2 * period + i] as f64).powi(2) / 256.0;
                }
                if e1 > e2 {
                    e1 = e2;
                }
                let mut decay = (0.5 * e1 / e2).sqrt() as f32;
                let mut offset = (HIST - pitch) as i64;
                let total = len + OVERLAP;
                let mut e = vec![0.0f32; total];
                let mut s1 = 0.0f64;
                for (i, ev) in e.iter_mut().enumerate() {
                    if offset + i as i64 >= HIST as i64 {
                        offset -= pitch as i64;
                        decay *= decay;
                    }
                    let at = (offset + i as i64).clamp(0, HIST as i64 - 1) as usize;
                    *ev = decay * exc[at] * fade;
                    s1 += (hist[at] as f64).powi(2) / 256.0;
                }
                // synthesis from the state of the last samples
                let mut mem: Vec<f64> = (0..LPC_ORDER).map(|k| hist[HIST - 1 - k] as f64).collect();
                let mut s2 = 0.0f64;
                for ev in e.iter_mut() {
                    let mut v = *ev as f64;
                    for (k, &a) in lpc.iter().enumerate() {
                        v -= a * mem[k];
                    }
                    mem.rotate_right(1);
                    mem[0] = v;
                    *ev = v as f32;
                    s2 += v * v / 256.0;
                }
                if !(s1 > 0.2 * s2) {
                    e.iter_mut().for_each(|v| *v = 0.0);
                } else if s1 < s2 {
                    let ratio = (((s1 * 0.5) + 1.0) / (s2 + 1.0)).sqrt() as f32;
                    e.iter_mut().for_each(|v| *v *= ratio);
                }
                // the part that overlaps the next frame, windowed so that the next frame's aliasing cancels it
                for i in 0..OVERLAP / 2 {
                    let tmp = self.window[i] * e[n + OVERLAP - 1 - i] + self.window[OVERLAP - i - 1] * e[n + i];
                    self.overlap_tail[ch][i] = self.window[OVERLAP - i - 1] * tmp;
                    self.overlap_tail[ch][OVERLAP - i - 1] = self.window[i] * tmp;
                }
                let samples: Vec<f32> = e[..n].to_vec();
                self.finish_channel(ch, &samples, &mut out, cc);
            }
        }
        self.loss_count += 1;
        out
    }

    /// The inverse MDCT of one channel (one long block, or several short ones whose coefficients are interleaved), windowed and
    /// added to the tail of the frame before; `out` gets the `n` samples of this frame.
    fn synthesise(&mut self, coef: &[f32], lm: usize, short_blocks: usize, ch: usize, out: &mut [f32]) {
        let n = coef.len();
        let (blocks, n2) = if short_blocks > 0 { (short_blocks, SHORT) } else { (1, n) };
        let imdct = &self.imdct[if short_blocks > 0 { 0 } else { lm }];
        let mut acc = vec![0.0f32; n + OVERLAP];
        let mut block = vec![0.0f32; n2];
        let mut raw = vec![0.0f32; 2 * n2];
        for b in 0..blocks {
            for (k, v) in block.iter_mut().enumerate() {
                *v = coef[b + k * blocks];
            }
            imdct.run(&block, &mut raw);
            // the part of the transform that is not zero in the low-overlap window: rise, flat, fall
            let skip = (n2 - OVERLAP) / 2;
            let base = n2 * b;
            for j in 0..(n2 + OVERLAP) {
                let w = if j < OVERLAP {
                    self.window[j]
                } else if j >= n2 {
                    self.window[n2 + OVERLAP - 1 - j]
                } else {
                    1.0
                };
                acc[base + j] += raw[skip + j] * w * IMDCT_SCALE;
            }
        }
        for j in 0..OVERLAP {
            acc[j] += self.overlap_tail[ch][j];
        }
        out.copy_from_slice(&acc[..n]);
        self.overlap_tail[ch].copy_from_slice(&acc[n..n + OVERLAP]);
    }

    /// Fills the blocks of a transient frame that came out empty with noise at the level the earlier frames had (section 4.3.5).
    fn anti_collapse(&mut self, x: &mut [f32], masks: &[[u8; 2]; NB_BANDS], lm: usize, c: usize, n: usize, start: usize, end: usize, pulses: &[i32; NB_BANDS]) {
        let mut seed = self.rng;
        for i in start..end {
            let n0 = band_width(i) as usize;
            let depth = (1 + pulses[i]) / ((n0 as i32) << lm);
            let thresh = 0.5 * (-0.125 * depth as f32).exp2();
            let sqrt_1 = 1.0 / ((n0 << lm) as f32).sqrt();
            for ch in 0..c {
                let mut prev1 = self.old_log_e[ch][i];
                let mut prev2 = self.old_log_e2[ch][i];
                if c == 1 {
                    prev1 = prev1.max(self.old_log_e[1][i]);
                    prev2 = prev2.max(self.old_log_e2[1][i]);
                }
                let ediff = (self.old_band_e[ch][i] - prev1.min(prev2)).max(0.0);
                let mut r = 2.0 * (-ediff).exp2();
                if lm == 3 {
                    r *= std::f32::consts::SQRT_2;
                }
                r = thresh.min(r) * sqrt_1;
                let base = ch * n + ((EBANDS[i] as usize) << lm);
                let mut renorm = false;
                for k in 0..(1usize << lm) {
                    if masks[i][ch] & (1 << k) == 0 {
                        for j in 0..n0 {
                            seed = lcg(seed);
                            x[base + (j << lm) + k] = if seed & 0x8000 != 0 { r } else { -r };
                        }
                        renorm = true;
                    }
                }
                if renorm {
                    renormalise(&mut x[base..base + (n0 << lm)], 1.0);
                }
            }
        }
    }
}

/// The scale between the unit-less inverse transform of `mdct.rs` and the sound the standard's decoder makes.
const IMDCT_SCALE: f32 = 1.0;

fn comb_filter(buf: &mut [f32], start: usize, len: usize, t0: i32, t1: i32, g0: f32, g1: f32, tap0: usize, tap1: usize, window: &[f32; OVERLAP]) {
    if g0 == 0.0 && g1 == 0.0 {
        return;
    }
    let (t0, t1) = (t0.max(MIN_PERIOD) as usize, t1.max(MIN_PERIOD) as usize);
    let g00 = g0 * COMB_GAINS[tap0][0];
    let g01 = g0 * COMB_GAINS[tap0][1];
    let g02 = g0 * COMB_GAINS[tap0][2];
    let g10 = g1 * COMB_GAINS[tap1][0];
    let g11 = g1 * COMB_GAINS[tap1][1];
    let g12 = g1 * COMB_GAINS[tap1][2];
    let ov = OVERLAP.min(len);
    for i in 0..len {
        let at = start + i;
        let x = buf[at];
        let tail = |t: usize, a: f32, b: f32, c: f32| a * buf[at - t] + b * (buf[at - t - 1] + buf[at - t + 1]) + c * (buf[at - t - 2] + buf[at - t + 2]);
        let y = if i < ov {
            let f = window[i] * window[i];
            x + (1.0 - f) * tail(t0, g00, g01, g02) + f * tail(t1, g10, g11, g12)
        } else {
            x + tail(t1, g10, g11, g12)
        };
        buf[at] = y;
    }
}

fn renormalise(x: &mut [f32], gain: f32) {
    let e: f32 = 1e-15 + x.iter().map(|v| v * v).sum::<f32>();
    let g = gain / e.sqrt();
    for v in x.iter_mut() {
        *v *= g;
    }
}

/// A number from the Laplace-like distribution the coarse energies follow: `fs` is the probability of zero and `decay` the rate.
fn laplace_decode(dec: &mut RangeDecoder<'_>, mut fs: u32, decay: u32) -> i32 {
    let fm = dec.decode_bin(15);
    let mut fl = 0u32;
    let mut val = 0i32;
    if fm >= fs {
        val = 1;
        fl = fs;
        fs = (((32768 - 32 - fs) as u64 * (16384 - decay) as u64) >> 15) as u32 + 1;
        while fs > 1 && fm >= fl + 2 * fs {
            fs *= 2;
            fl += fs;
            fs = (((fs - 2) as u64 * decay as u64) >> 15) as u32 + 1;
            val += 1;
        }
        if fs <= 1 {
            let di = (fm - fl) >> 1;
            val += di as i32;
            fl += 2 * di;
        }
        if fm < fl + fs {
            val = -val;
        } else {
            fl += fs;
        }
    }
    dec.update(fl, (fl + fs).min(32768), 32768);
    val
}

fn unquant_coarse_energy(dec: &mut RangeDecoder<'_>, start: usize, end: usize, old: &mut [[f32; NB_BANDS]; 2], intra: bool, c: usize, lm: usize) {
    let model = &E_PROB_MODEL[(lm * 2 + intra as usize) * 42..][..42];
    let (coef, beta) = if intra { (0.0, BETA_INTRA) } else { (PRED_COEF[lm], BETA_COEF[lm]) };
    let budget = dec.storage() as i32 * 8;
    let mut prev = [0.0f32; 2];
    for i in start..end {
        for ch in 0..c {
            let tell = dec.tell();
            let qi = if budget - tell >= 15 {
                let pi = 2 * i.min(20);
                laplace_decode(dec, (model[pi] as u32) << 7, (model[pi + 1] as u32) << 6)
            } else if budget - tell >= 2 {
                let q = dec.icdf(&SMALL_ENERGY_ICDF, 2) as i32;
                (q >> 1) ^ -(q & 1)
            } else if budget - tell >= 1 {
                -(dec.bit_logp(1) as i32)
            } else {
                -1
            };
            let q = qi as f32;
            let o = old[ch][i].max(-9.0);
            old[ch][i] = coef * o + prev[ch] + q;
            prev[ch] = prev[ch] + q - beta * q;
        }
    }
}

fn tf_decode(dec: &mut RangeDecoder<'_>, start: usize, end: usize, transient: bool, lm: usize) -> [i32; NB_BANDS] {
    let mut budget = dec.storage() as i32 * 8;
    let mut tell = dec.tell();
    let mut logp = if transient { 2u32 } else { 4 };
    let select_rsv = lm > 0 && tell + logp as i32 + 1 <= budget;
    budget -= select_rsv as i32;
    let (mut changed, mut curr) = (0, 0);
    let mut res = [0i32; NB_BANDS];
    for r in res.iter_mut().take(end).skip(start) {
        if tell + logp as i32 <= budget {
            curr ^= dec.bit_logp(logp) as i32;
            tell = dec.tell();
            changed |= curr;
        }
        *r = curr;
        logp = if transient { 4 } else { 5 };
    }
    let t = transient as usize;
    let mut select = 0;
    if select_rsv && TF_SELECT[lm][4 * t + changed as usize] != TF_SELECT[lm][4 * t + 2 + changed as usize] {
        select = dec.bit_logp(1) as usize;
    }
    for r in res.iter_mut().take(end).skip(start) {
        *r = TF_SELECT[lm][4 * t + 2 * select + *r as usize] as i32;
    }
    res
}

/// Divides the bits of the frame among the bands (section 4.3.3), reading the skip, intensity and dual stereo decisions on the way.
#[allow(clippy::too_many_arguments)]
fn compute_allocation(dec: &mut RangeDecoder<'_>, start: usize, end: usize, offsets: &[i32; NB_BANDS], cap: &[i32; NB_BANDS], alloc_trim: i32, total: i32, c: usize, lm: i32) -> Allocation {
    let ci = c as i32;
    let mut total = total.max(0);
    let mut skip_start = start;
    let skip_rsv = if total >= 1 << BITRES { 1 << BITRES } else { 0 };
    total -= skip_rsv;
    let (mut intensity_rsv, mut dual_rsv) = (0, 0);
    if c == 2 {
        intensity_rsv = LOG2_FRAC[end - start] as i32;
        if intensity_rsv > total {
            intensity_rsv = 0;
        } else {
            total -= intensity_rsv;
            dual_rsv = if total >= 1 << BITRES { 1 << BITRES } else { 0 };
            total -= dual_rsv;
        }
    }
    let mut thresh = [0i32; NB_BANDS];
    let mut trim_offset = [0i32; NB_BANDS];
    for j in start..end {
        let w = band_width(j);
        thresh[j] = (ci << BITRES).max((3 * w << lm << BITRES) >> 4);
        trim_offset[j] = ci * w * (alloc_trim - 5 - lm) * (end as i32 - j as i32 - 1) * (1 << (lm + BITRES)) >> 6;
        if (w << lm) == 1 {
            trim_offset[j] -= ci << BITRES;
        }
    }
    let vec_bits = |v: usize, j: usize| -> i32 { (ci * band_width(j) * BAND_ALLOCATION[v * NB_BANDS + j] as i32) << lm >> 2 };
    let (mut lo, mut hi) = (1i32, 10i32);
    while lo <= hi {
        let mid = (lo + hi) >> 1;
        let mut done = false;
        let mut psum = 0;
        for j in (start..end).rev() {
            let mut b = vec_bits(mid as usize, j);
            if b > 0 {
                b = (b + trim_offset[j]).max(0);
            }
            b += offsets[j];
            if b >= thresh[j] || done {
                done = true;
                psum += b.min(cap[j]);
            } else if b >= ci << BITRES {
                psum += ci << BITRES;
            }
        }
        if psum > total {
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    let hi = lo;
    let lo = lo - 1;
    let mut bits1 = [0i32; NB_BANDS];
    let mut bits2 = [0i32; NB_BANDS];
    for j in start..end {
        let mut b1 = vec_bits(lo as usize, j);
        let mut b2 = if hi >= 11 { cap[j] } else { vec_bits(hi as usize, j) };
        if b1 > 0 {
            b1 = (b1 + trim_offset[j]).max(0);
        }
        if b2 > 0 {
            b2 = (b2 + trim_offset[j]).max(0);
        }
        if lo > 0 {
            b1 += offsets[j];
        }
        b2 += offsets[j];
        if offsets[j] > 0 {
            skip_start = j;
        }
        bits1[j] = b1;
        bits2[j] = (b2 - b1).max(0);
    }
    // interpolate between the two table rows
    let alloc_floor = ci << BITRES;
    let stereo = (c > 1) as i32;
    let (mut lo, mut hi) = (0i32, 1i32 << 6);
    for _ in 0..6 {
        let mid = (lo + hi) >> 1;
        let mut psum = 0;
        let mut done = false;
        for j in (start..end).rev() {
            let tmp = bits1[j] + ((mid * bits2[j]) >> 6);
            if tmp >= thresh[j] || done {
                done = true;
                psum += tmp.min(cap[j]);
            } else if tmp >= alloc_floor {
                psum += alloc_floor;
            }
        }
        if psum > total {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let mut bits = [0i32; NB_BANDS];
    let mut psum = 0;
    let mut done = false;
    for j in (start..end).rev() {
        let mut tmp = bits1[j] + ((lo * bits2[j]) >> 6);
        if tmp < thresh[j] && !done {
            tmp = if tmp >= alloc_floor { alloc_floor } else { 0 };
        } else {
            done = true;
        }
        tmp = tmp.min(cap[j]);
        bits[j] = tmp;
        psum += tmp;
    }
    // work back from the top band, reading the decision to skip each one
    let mut coded = end as i32;
    loop {
        let j = (coded - 1) as usize;
        if coded - 1 <= skip_start as i32 {
            total += skip_rsv;
            break;
        }
        let left = total - psum;
        let span = (EBANDS[coded as usize] - EBANDS[start]) as i32;
        let percoeff = left / span;
        let left = left - span * percoeff;
        let rem = (left - (EBANDS[j] - EBANDS[start]) as i32).max(0);
        let bw = (EBANDS[coded as usize] - EBANDS[j]) as i32;
        let mut band_bits = bits[j] + percoeff * bw + rem;
        if band_bits >= thresh[j].max(alloc_floor + (1 << BITRES)) {
            if dec.bit_logp(1) {
                break;
            }
            psum += 1 << BITRES;
            band_bits -= 1 << BITRES;
        }
        psum -= bits[j] + intensity_rsv;
        if intensity_rsv > 0 {
            intensity_rsv = LOG2_FRAC[j - start] as i32;
        }
        psum += intensity_rsv;
        if band_bits >= alloc_floor {
            psum += alloc_floor;
            bits[j] = alloc_floor;
        } else {
            bits[j] = 0;
        }
        coded -= 1;
    }
    let intensity = if intensity_rsv > 0 { start as i32 + dec.uint((coded + 1 - start as i32) as u32) as i32 } else { 0 };
    if intensity <= start as i32 {
        total += dual_rsv;
        dual_rsv = 0;
    }
    let dual_stereo = if dual_rsv > 0 { dec.bit_logp(1) } else { false };
    // hand out what is left, per coefficient
    let left = total - psum;
    let span = (EBANDS[coded as usize] - EBANDS[start]) as i32;
    let percoeff = left / span;
    let mut left = left - span * percoeff;
    for j in start..coded as usize {
        bits[j] += percoeff * band_width(j);
    }
    for j in start..coded as usize {
        let t = left.min(band_width(j));
        bits[j] += t;
        left -= t;
    }
    let mut fine_bits = [0i32; NB_BANDS];
    let mut fine_priority = [0i32; NB_BANDS];
    let mut balance = 0;
    let mut j = start;
    while j < coded as usize {
        let n0 = band_width(j);
        let nn = n0 << lm;
        bits[j] += balance;
        let excess;
        if nn > 1 {
            let ex = (bits[j] - cap[j]).max(0);
            bits[j] -= ex;
            let den = ci * nn + if c == 2 && nn > 2 && !dual_stereo && (j as i32) < intensity { 1 } else { 0 };
            let nclogn = den * (LOG_N[j] as i32 + (lm << BITRES));
            let mut offset = (nclogn >> 1) - den * FINE_OFFSET;
            if nn == 2 {
                offset += (den << BITRES) >> 2;
            }
            if bits[j] + offset < (den * 2) << BITRES {
                offset += nclogn >> 2;
            } else if bits[j] + offset < (den * 3) << BITRES {
                offset += nclogn >> 3;
            }
            fine_bits[j] = ((bits[j] + offset + (den << (BITRES - 1))) / (den << BITRES)).max(0);
            if ci * fine_bits[j] > (bits[j] >> BITRES) {
                fine_bits[j] = (bits[j] >> stereo) >> BITRES;
            }
            fine_bits[j] = fine_bits[j].min(MAX_FINE_BITS);
            fine_priority[j] = (fine_bits[j] * (den << BITRES) >= bits[j] + offset) as i32;
            bits[j] -= (ci * fine_bits[j]) << BITRES;
            excess = ex;
        } else {
            excess = (bits[j] - (ci << BITRES)).max(0);
            bits[j] -= excess;
            fine_bits[j] = 0;
            fine_priority[j] = 1;
        }
        let mut excess = excess;
        if excess > 0 {
            let extra_fine = (excess >> (stereo + BITRES)).min(MAX_FINE_BITS - fine_bits[j]);
            fine_bits[j] += extra_fine;
            let extra_bits = (extra_fine * ci) << BITRES;
            fine_priority[j] = (extra_bits >= excess - balance) as i32;
            excess -= extra_bits;
        }
        balance = excess;
        j += 1;
    }
    while j < end {
        fine_bits[j] = (bits[j] >> stereo) >> BITRES;
        bits[j] = 0;
        fine_priority[j] = (fine_bits[j] < 1) as i32;
        j += 1;
    }
    Allocation { pulses: bits, fine_bits, fine_priority, intensity, dual_stereo, balance, coded_bands: coded }
}

// ---- the band shapes

/// Reads the shape of every band.
struct BandDecoder<'a, 'b> {
    dec: &'a mut RangeDecoder<'b>,
    lm: i32,
    seed: u32,
    spread: i32,
    /// Eighths of a bit still free for the bands after the current one.
    remaining: i32,
}

/// Rotates pairs of values `stride` apart to spread a few pulses over the band.
fn rotate_pass(x: &mut [f32], stride: usize, c: f32, s: f32) {
    let len = x.len();
    if len <= stride {
        return;
    }
    for i in 0..len - stride {
        let (x1, x2) = (x[i], x[i + stride]);
        x[i + stride] = c * x2 + s * x1;
        x[i] = c * x1 - s * x2;
    }
    if len >= 2 * stride + 1 {
        for i in (0..=len - 2 * stride - 1).rev() {
            let (x1, x2) = (x[i], x[i + stride]);
            x[i + stride] = c * x2 + s * x1;
            x[i] = c * x1 - s * x2;
        }
    }
}

fn exp_rotation(x: &mut [f32], len: usize, stride: usize, k: i32, spread: i32) {
    if 2 * k as usize >= len || spread == SPREAD_NONE || stride == 0 {
        return;
    }
    let factor = [15.0f32, 10.0, 5.0][(spread.clamp(1, 3) - 1) as usize];
    let gain = len as f32 / (len as f32 + factor * k as f32);
    let theta = 0.5 * gain * gain;
    let c = (0.5 * std::f32::consts::PI * theta).cos();
    let s = (0.5 * std::f32::consts::PI * (1.0 - theta)).cos();
    let mut stride2 = 0usize;
    if len >= 8 * stride {
        stride2 = 1;
        while (stride2 * stride2 + stride2) * stride + (stride >> 2) < len {
            stride2 += 1;
        }
    }
    let per = len / stride;
    for i in 0..stride {
        let blk = &mut x[i * per..(i + 1) * per];
        if stride2 > 0 {
            rotate_pass(blk, stride2, s, c);
        }
        rotate_pass(blk, 1, c, s);
    }
}

/// The pulse counts `u[n][k]` of the pyramid vector quantiser, row by row for 1 to `n` dimensions: there are `u[n][k] + u[n][k + 1]`
/// vectors of `n` dimensions with `k` pulses (signs included). `u[n][k] = u[n-1][k] + u[n][k-1] + u[n-1][k-1]`.
fn pvq_rows(n: usize, k: usize) -> Vec<Vec<u32>> {
    let mut rows: Vec<Vec<u32>> = Vec::with_capacity(n);
    let mut row = vec![0u32; k + 2];
    for r in row.iter_mut().skip(1) {
        *r = 1;
    }
    rows.push(row.clone());
    for _ in 2..=n {
        let mut next = vec![0u32; k + 2];
        for kk in 1..k + 2 {
            next[kk] = row[kk].wrapping_add(next[kk - 1]).wrapping_add(row[kk - 1]);
        }
        rows.push(next.clone());
        row = next;
    }
    rows
}

/// The `index`-th pulse vector of `n` dimensions with `k` pulses, in the standard's enumeration order.
fn pvq_unrank(rows: &[Vec<u32>], k: usize, mut index: u32, y: &mut [i32]) {
    let n = y.len();
    let mut kk = k;
    for (j, yj) in y.iter_mut().enumerate() {
        let u = &rows[n - 1 - j];
        let p = u[kk + 1];
        let negative = index >= p;
        if negative {
            index -= p;
        }
        let was = kk;
        while kk > 0 && u[kk] > index {
            kk -= 1;
        }
        index -= u[kk];
        let v = (was - kk) as i32;
        *yj = if negative { -v } else { v };
    }
}

fn haar1(x: &mut [f32], n0: usize, stride: usize) {
    let half = n0 >> 1;
    let r = std::f32::consts::FRAC_1_SQRT_2;
    for i in 0..stride {
        for j in 0..half {
            let t1 = r * x[stride * 2 * j + i];
            let t2 = r * x[stride * (2 * j + 1) + i];
            x[stride * 2 * j + i] = t1 + t2;
            x[stride * (2 * j + 1) + i] = t1 - t2;
        }
    }
}

fn hadamard_slot(stride: usize, i: usize, hadamard: bool) -> usize {
    if hadamard && (2..=16).contains(&stride) {
        ORDERY[stride - 2 + i]
    } else {
        i
    }
}

fn deinterleave_hadamard(x: &mut [f32], n0: usize, stride: usize, hadamard: bool) {
    let n = n0 * stride;
    if x.len() < n {
        return;
    }
    let mut tmp = vec![0.0f32; n];
    for i in 0..stride {
        for j in 0..n0 {
            tmp[hadamard_slot(stride, i, hadamard) * n0 + j] = x[j * stride + i];
        }
    }
    x[..n].copy_from_slice(&tmp);
}

fn interleave_hadamard(x: &mut [f32], n0: usize, stride: usize, hadamard: bool) {
    let n = n0 * stride;
    if x.len() < n {
        return;
    }
    let mut tmp = vec![0.0f32; n];
    for i in 0..stride {
        for j in 0..n0 {
            tmp[j * stride + i] = x[hadamard_slot(stride, i, hadamard) * n0 + j];
        }
    }
    x[..n].copy_from_slice(&tmp);
}

fn compute_qn(n: i32, b: i32, offset: i32, pulse_cap: i32, stereo: bool) -> i32 {
    const EXP2_TABLE8: [i32; 8] = [16384, 17866, 19483, 21247, 23170, 25267, 27554, 30048];
    let mut n2 = 2 * n - 1;
    if stereo && n == 2 {
        n2 -= 1;
    }
    let mut qb = (b - pulse_cap - (4 << BITRES)).min((b + n2 * offset) / n2);
    qb = qb.min(8 << BITRES);
    if qb < (1 << BITRES >> 1) {
        1
    } else {
        let qn = EXP2_TABLE8[(qb & 7) as usize] >> (14 - (qb >> BITRES)).clamp(0, 31);
        (qn + 1) >> 1 << 1
    }
}

/// Turns a mid and a side shape into left and right, each of unit energy.
fn stereo_merge(x: &mut [f32], y: &mut [f32], mid: f32) {
    let (mut xp, mut side) = (0.0f32, 0.0f32);
    for (a, b) in x.iter().zip(y.iter()) {
        xp += a * b;
        side += b * b;
    }
    xp *= mid;
    let el = mid * mid + side - 2.0 * xp;
    let er = mid * mid + side + 2.0 * xp;
    if er < 6e-4 || el < 6e-4 {
        y.copy_from_slice(x);
        return;
    }
    let (lgain, rgain) = (1.0 / el.sqrt(), 1.0 / er.sqrt());
    for (a, b) in x.iter_mut().zip(y.iter_mut()) {
        let l = mid * *a;
        let r = *b;
        *a = lgain * (l - r);
        *b = rgain * (l + r);
    }
}

/// What a call of [`BandDecoder::quant_band`] is told about the band besides its values.
#[derive(Clone, Copy)]
struct BandCtx {
    band: usize,
    lm: i32,
    intensity: i32,
    tf_change: i32,
    level: i32,
}

impl BandDecoder<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    fn all_bands(&mut self, start: usize, end: usize, x: &mut [f32], mut y: Option<&mut [f32]>, masks: &mut [[u8; 2]; NB_BANDS], al: &Allocation, short: bool, tf_res: &[i32; NB_BANDS], total_bits: i32) {
        let lm = self.lm;
        let mm = 1usize << lm;
        let c = if y.is_some() { 2 } else { 1 };
        let blocks = if short { mm as i32 } else { 1 };
        let nlen = mm * EBANDS[NB_BANDS] as usize;
        let mut norm = vec![0.0f32; c * nlen];
        let mut balance = al.balance;
        let mut lowband_offset = 0usize;
        let mut update_lowband = true;
        let mut dual_stereo = al.dual_stereo;
        for i in start..end {
            let n = mm * band_width(i) as usize;
            let lo = mm * EBANDS[i] as usize;
            let tell = self.dec.tell_frac();
            if i != start {
                balance -= tell;
            }
            self.remaining = total_bits - tell - 1;
            let b = if (i as i32) < al.coded_bands {
                let curr_balance = balance / 3.min(al.coded_bands - i as i32);
                0.max(16383.min((self.remaining + 1).min(al.pulses[i] + curr_balance)))
            } else {
                0
            };
            if (lo >= n + mm * EBANDS[start] as usize || i == start + 1) && (update_lowband || lowband_offset == 0) {
                lowband_offset = i;
            }
            if i == start + 1 {
                // hybrid folding: the second band folds from the first, which is narrower, so the tail of the first is repeated to give
                // it enough to fold from (nothing is copied in CELT-only streams, where the first two bands are equally wide)
                let n1 = mm * band_width(start) as usize;
                let n2 = mm * band_width(start + 1) as usize;
                if n2 > n1 {
                    let base = mm * EBANDS[start] as usize;
                    norm.copy_within(base + 2 * n1 - n2..base + n1, base + n1);
                    if dual_stereo {
                        norm.copy_within(nlen + base + 2 * n1 - n2..nlen + base + n1, nlen + base + n1);
                    }
                }
            }
            let tf_change = tf_res[i];
            let mut eff: i64 = -1;
            let (mut x_cm, mut y_cm);
            if lowband_offset != 0 && (self.spread != SPREAD_AGGRESSIVE || blocks > 1 || tf_change < 0) {
                eff = ((mm * EBANDS[start] as usize) as i64).max((mm * EBANDS[lowband_offset] as usize) as i64 - n as i64);
                let mut fold_start = lowband_offset;
                while fold_start > 0 {
                    fold_start -= 1;
                    if (mm * EBANDS[fold_start] as usize) as i64 <= eff {
                        break;
                    }
                }
                let mut fold_end = lowband_offset;
                while fold_end < i && ((mm * EBANDS[fold_end] as usize) as i64) < eff + n as i64 {
                    fold_end += 1;
                }
                x_cm = 0u32;
                y_cm = 0u32;
                for fi in fold_start..fold_end {
                    x_cm |= masks[fi][0] as u32;
                    y_cm |= masks[fi][c - 1] as u32;
                }
            } else {
                x_cm = (1u32 << blocks) - 1;
                y_cm = x_cm;
            }
            if dual_stereo && i as i32 == al.intensity {
                dual_stereo = false;
                for j in mm * EBANDS[start] as usize..lo {
                    norm[j] = 0.5 * (norm[j] + norm[nlen + j]);
                }
            }
            let ctx = BandCtx { band: i, lm, intensity: al.intensity, tf_change, level: 0 };
            let take = |norm: &[f32], base: usize| -> Option<Vec<f32>> { (eff >= 0).then(|| norm.get(base + eff as usize..base + eff as usize + n).map(|s| s.to_vec())).flatten() };
            if dual_stereo {
                let lb1 = take(&norm, 0);
                let lb2 = take(&norm, nlen);
                x_cm = self.quant_band(ctx, &mut x[lo..lo + n], None, b / 2, blocks, lb1.as_deref(), Some(&mut norm[lo..lo + n]), 1.0, x_cm);
                let ys = &mut y.as_mut().unwrap()[lo..lo + n];
                y_cm = self.quant_band(ctx, ys, None, b / 2, blocks, lb2.as_deref(), Some(&mut norm[nlen + lo..nlen + lo + n]), 1.0, y_cm);
            } else {
                let lb = take(&norm, 0);
                let yy = y.as_mut().map(|v| &mut v[lo..lo + n]);
                x_cm = self.quant_band(ctx, &mut x[lo..lo + n], yy, b, blocks, lb.as_deref(), Some(&mut norm[lo..lo + n]), 1.0, x_cm | y_cm);
                y_cm = x_cm;
            }
            masks[i][0] = x_cm as u8;
            masks[i][c - 1] = y_cm as u8;
            balance += al.pulses[i] + tell;
            update_lowband = b > ((n as i32) << BITRES);
        }
    }

    /// Reads one band, or half of one, or (stereo) the two channels of one. `x` (and `y`) hold `n` values. Returns the collapse mask:
    /// which of the short blocks of the band got any energy.
    #[allow(clippy::too_many_arguments)]
    fn quant_band(&mut self, ctx: BandCtx, x: &mut [f32], y: Option<&mut [f32]>, mut b: i32, blocks: i32, lowband: Option<&[f32]>, mut lowband_out: Option<&mut [f32]>, gain: f32, mut fill: u32) -> u32 {
        let BandCtx { band, mut lm, intensity, level, .. } = ctx;
        let mut tf_change = ctx.tf_change;
        let n0 = x.len() as i32;
        let mut n = n0;
        let mut n_b = n / blocks.max(1);
        let mut bb = blocks;
        let mut b0 = blocks;
        let long_blocks = b0 == 1;
        let stereo = y.is_some();
        let mut time_divide = 0;
        let mut recombine = 0;
        let mut inv = false;
        let mut cm: u32 = 0;
        if n == 1 {
            let one = |v: &mut [f32], dec: &mut RangeDecoder<'_>, remaining: &mut i32| {
                let mut sign = 0;
                if *remaining >= 1 << BITRES {
                    sign = dec.bits(1);
                    *remaining -= 1 << BITRES;
                }
                v[0] = if sign != 0 { -1.0 } else { 1.0 };
            };
            one(x, self.dec, &mut self.remaining);
            if let Some(yy) = y {
                one(yy, self.dec, &mut self.remaining);
            }
            if let Some(o) = lowband_out.as_deref_mut() {
                o[0] = x[0];
            }
            return 1;
        }
        let mut lb: Option<Vec<f32>> = lowband.map(|s| s.to_vec());
        if !stereo && level == 0 {
            if tf_change > 0 {
                recombine = tf_change;
            }
            for k in 0..recombine {
                const BIT_INTERLEAVE: [u32; 16] = [0, 1, 1, 1, 2, 3, 3, 3, 2, 3, 3, 3, 2, 3, 3, 3];
                if let Some(l) = lb.as_mut() {
                    haar1(l, (n >> k) as usize, 1 << k);
                }
                fill = BIT_INTERLEAVE[(fill & 15) as usize] | (BIT_INTERLEAVE[((fill >> 4) & 15) as usize] << 2);
            }
            bb >>= recombine;
            n_b <<= recombine;
            while n_b & 1 == 0 && tf_change < 0 {
                if let Some(l) = lb.as_mut() {
                    haar1(l, n_b as usize, bb as usize);
                }
                fill |= fill << bb;
                bb <<= 1;
                n_b >>= 1;
                time_divide += 1;
                tf_change += 1;
            }
            b0 = bb;
            if b0 > 1 {
                if let Some(l) = lb.as_mut() {
                    deinterleave_hadamard(l, (n_b >> recombine) as usize, (b0 << recombine) as usize, long_blocks);
                }
            }
        }
        let n_b0 = n_b;
        // a band that needs more than 1.5 bit beyond what one pyramid can code is split in two halves
        let cache = cache_row((lm + 1).max(0) as usize, band);
        let mut split = stereo;
        let mut half = 0usize;
        if !stereo && lm != -1 && !cache.is_empty() && b > cache[cache[0] as usize] as i32 + 12 && n > 2 {
            n >>= 1;
            half = n as usize;
            split = true;
            lm -= 1;
            if bb == 1 {
                fill = (fill & 1) | (fill << 1);
            }
            bb = (bb + 1) >> 1;
        }
        let mid: f32;
        if split {
            let pulse_cap = LOG_N[band] as i32 + lm * (1 << BITRES);
            let offset = (pulse_cap >> 1) - if stereo && n == 2 { QTHETA_OFFSET_TWOPHASE } else { QTHETA_OFFSET };
            let mut qn = compute_qn(n, b, offset, pulse_cap, stereo);
            if stereo && band as i32 >= intensity {
                qn = 1;
            }
            let tell = self.dec.tell_frac();
            let mut itheta;
            if qn != 1 {
                if stereo && n > 2 {
                    let p0 = 3;
                    let x0 = qn / 2;
                    let ft = p0 * (x0 + 1) + x0;
                    let fs = self.dec.decode(ft as u32) as i32;
                    let xv = if fs < (x0 + 1) * p0 { fs / p0 } else { x0 + 1 + (fs - (x0 + 1) * p0) };
                    let (fl, fh) = if xv <= x0 { (p0 * xv, p0 * (xv + 1)) } else { ((xv - 1 - x0) + (x0 + 1) * p0, (xv - x0) + (x0 + 1) * p0) };
                    self.dec.update(fl as u32, fh as u32, ft as u32);
                    itheta = xv;
                } else if b0 > 1 || stereo {
                    itheta = self.dec.uint((qn + 1) as u32) as i32;
                } else {
                    let ft = ((qn >> 1) + 1) * ((qn >> 1) + 1);
                    let fm = self.dec.decode(ft as u32) as i32;
                    let (fs, fl);
                    if fm < ((qn >> 1) * ((qn >> 1) + 1) >> 1) {
                        itheta = (isqrt((8 * fm + 1) as u32) as i32 - 1) >> 1;
                        fs = itheta + 1;
                        fl = itheta * (itheta + 1) >> 1;
                    } else {
                        itheta = (2 * (qn + 1) - isqrt((8 * (ft - fm - 1) + 1) as u32) as i32) >> 1;
                        fs = qn + 1 - itheta;
                        fl = ft - ((qn + 1 - itheta) * (qn + 2 - itheta) >> 1);
                    }
                    self.dec.update(fl as u32, (fl + fs) as u32, ft as u32);
                }
                itheta = (itheta as i64 * 16384 / qn as i64) as i32;
            } else {
                if stereo && b > 2 << BITRES && self.remaining > 2 << BITRES {
                    inv = self.dec.bit_logp(2);
                }
                itheta = 0;
            }
            let qalloc = self.dec.tell_frac() - tell;
            b -= qalloc;
            let orig_fill = fill;
            let (imid, iside, mut delta);
            if itheta == 0 {
                imid = 32767;
                iside = 0;
                fill &= (1u32 << bb) - 1;
                delta = -16384;
            } else if itheta == 16384 {
                imid = 0;
                iside = 32767;
                fill &= ((1u32 << bb) - 1) << bb;
                delta = 16384;
            } else {
                imid = bitexact_cos(itheta);
                iside = bitexact_cos(16384 - itheta);
                delta = frac_mul16((n - 1) << 7, bitexact_log2tan(iside, imid));
            }
            mid = imid as f32 / 32768.0;
            let side = iside as f32 / 32768.0;
            let child = BandCtx { band, lm, intensity, tf_change, level: if stereo { 0 } else { level + 1 } };
            if n == 2 && stereo {
                // mid and side are orthogonal, so the side needs just its sign
                let mut mbits = b;
                let mut sbits = 0;
                if itheta != 0 && itheta != 16384 {
                    sbits = 1 << BITRES;
                }
                mbits -= sbits;
                let swap = itheta > 8192;
                self.remaining -= qalloc + sbits;
                let mut sign = 0;
                if sbits > 0 {
                    sign = self.dec.bits(1);
                }
                let sign = 1.0 - 2.0 * sign as f32;
                let y = y.unwrap();
                let (x2, y2) = if swap { (&mut *y, &mut *x) } else { (&mut *x, &mut *y) };
                let own = BandCtx { band, lm, intensity, tf_change, level };
                cm = self.quant_band(own, &mut *x2, None, mbits, bb, lb.as_deref(), lowband_out.take(), gain, orig_fill);
                y2[0] = -sign * x2[1];
                y2[1] = sign * x2[0];
                // x and y get mid * x and side * y; the pair becomes the difference and the sum
                let (xo, yo) = if swap { (y2, x2) } else { (x2, y2) };
                for k in 0..2 {
                    let (m, s) = (mid * xo[k], side * yo[k]);
                    xo[k] = m - s;
                    yo[k] = m + s;
                }
                if inv {
                    yo[0] = -yo[0];
                    yo[1] = -yo[1];
                }
                return cm;
            }
            if b0 > 1 && !stereo && (itheta & 0x3fff) != 0 {
                if itheta > 8192 {
                    delta -= delta >> (4 - lm);
                } else {
                    delta = 0.min(delta + ((n << BITRES) >> (5 - lm)));
                }
            }
            let mut mbits = 0.max(b.min((b - delta) / 2));
            let mut sbits = b - mbits;
            self.remaining -= qalloc;
            let mut rebalance = self.remaining;
            let lb_ref = lb.as_deref();
            let gain_x = if stereo { 1.0 } else { gain * mid };
            let gain_y = gain * side;
            let shift_y = if stereo { 0 } else { (b0 >> 1) as u32 };
            let fill_y = fill >> bb;
            let out_x = if stereo { lowband_out.take() } else { None };
            // the first part is the mid (the first half of the band, or the first channel), the second the side
            let (mut xa, mut xb): (&mut [f32], &mut [f32]);
            let mut yy_store = y;
            if stereo {
                xa = &mut *x;
                xb = yy_store.take().unwrap();
            } else {
                let (a, h) = x.split_at_mut(half);
                xa = a;
                xb = h;
            }
            let lb_y = if stereo { None } else { lb_ref.and_then(|l| l.get(half..)) };
            let ctx_x = child;
            if mbits >= sbits {
                cm = self.quant_band(ctx_x, &mut *xa, None, mbits, bb, lb_ref, out_x, gain_x, fill);
                rebalance = mbits - (rebalance - self.remaining);
                if rebalance > 3 << BITRES && itheta != 0 {
                    sbits += rebalance - (3 << BITRES);
                }
                cm |= self.quant_band(ctx_x, &mut *xb, None, sbits, bb, lb_y, None, gain_y, fill_y) << shift_y;
            } else {
                cm = self.quant_band(ctx_x, &mut *xb, None, sbits, bb, lb_y, None, gain_y, fill_y) << shift_y;
                rebalance = sbits - (rebalance - self.remaining);
                if rebalance > 3 << BITRES && itheta != 16384 {
                    mbits += rebalance - (3 << BITRES);
                }
                cm |= self.quant_band(ctx_x, &mut *xa, None, mbits, bb, lb_ref, out_x, gain_x, fill);
            }
            if stereo {
                stereo_merge(xa, xb, mid);
                if inv {
                    xb.iter_mut().for_each(|v| *v = -*v);
                }
            }
            (xa, xb) = (&mut [][..], &mut [][..]);
            let _ = (xa, xb);
        } else {
            // one pyramid
            let mut q = bits_to_pulses(band, lm, b);
            let mut curr_bits = pulses_to_bits(band, lm, q);
            self.remaining -= curr_bits;
            while self.remaining < 0 && q > 0 {
                self.remaining += curr_bits;
                q -= 1;
                curr_bits = pulses_to_bits(band, lm, q);
                self.remaining -= curr_bits;
            }
            let nn = n as usize;
            if q != 0 {
                cm = self.unquant_pulses(&mut x[..nn], pulses_of(q), bb, gain);
            } else {
                // no pulse: fill the band from lower bands or with noise
                let cm_mask = (1u32 << bb) - 1;
                fill &= cm_mask;
                if fill == 0 {
                    x[..nn].iter_mut().for_each(|v| *v = 0.0);
                } else {
                    match lb.as_deref() {
                        None => {
                            for v in x[..nn].iter_mut() {
                                self.seed = lcg(self.seed);
                                *v = ((self.seed as i32) >> 20) as f32;
                            }
                            cm = cm_mask;
                        }
                        Some(l) => {
                            for (j, v) in x[..nn].iter_mut().enumerate() {
                                self.seed = lcg(self.seed);
                                let t = 1.0 / 256.0;
                                *v = l.get(j).copied().unwrap_or(0.0) + if self.seed & 0x8000 != 0 { t } else { -t };
                            }
                            cm = fill;
                        }
                    }
                    renormalise(&mut x[..nn], gain);
                }
            }
        }
        if !stereo && level == 0 {
            // undo the reorganisation of this band into time order, and the time and frequency changes
            if b0 > 1 {
                interleave_hadamard(x, (n_b >> recombine) as usize, (b0 << recombine) as usize, long_blocks);
            }
            let (mut nb, mut bq) = (n_b0, b0);
            for _ in 0..time_divide {
                bq >>= 1;
                nb <<= 1;
                cm |= cm >> bq;
                haar1(x, nb as usize, bq as usize);
            }
            for k in 0..recombine {
                const DEINTERLEAVE: [u32; 16] = [0x00, 0x03, 0x0C, 0x0F, 0x30, 0x33, 0x3C, 0x3F, 0xC0, 0xC3, 0xCC, 0xCF, 0xF0, 0xF3, 0xFC, 0xFF];
                cm = DEINTERLEAVE[(cm & 15) as usize];
                haar1(x, (n0 >> k) as usize, 1 << k);
            }
            bq <<= recombine;
            if let Some(o) = lowband_out.as_deref_mut() {
                let s = (n0 as f32).sqrt();
                for (ov, xv) in o.iter_mut().zip(x.iter()) {
                    *ov = s * xv;
                }
            }
            cm &= (1u32 << bq) - 1;
        }
        cm
    }

    /// Reads the index of a pulse vector and makes the unit-norm shape from it.
    fn unquant_pulses(&mut self, x: &mut [f32], k: i32, blocks: i32, gain: f32) -> u32 {
        let n = x.len();
        let k = k.clamp(1, 1 << 16) as usize;
        let rows = pvq_rows(n, k);
        let total = rows[n - 1][k].wrapping_add(rows[n - 1][k + 1]);
        let index = self.dec.uint(total.max(2));
        let mut iy = vec![0i32; n];
        pvq_unrank(&rows, k, index, &mut iy);
        let ryy: f32 = iy.iter().map(|&v| (v as f32) * (v as f32)).sum();
        let g = gain / ryy.max(1.0).sqrt();
        for (xv, &yv) in x.iter_mut().zip(&iy) {
            *xv = g * yv as f32;
        }
        exp_rotation(x, n, blocks.max(1) as usize, k as i32, self.spread);
        if blocks <= 1 {
            return 1;
        }
        let n0 = n / blocks as usize;
        let mut mask = 0u32;
        for i in 0..blocks as usize {
            if iy[i * n0..(i + 1) * n0].iter().any(|&v| v != 0) {
                mask |= 1 << i;
            }
        }
        mask
    }
}

/// Decodes the CELT frames of an Opus file (all packets must be CELT), as the decoder of a player would: 48 kHz, interleaved.
pub fn decode_celt_packets(head: &crate::opus::Head, packets: &[Vec<u8>]) -> Result<Vec<f32>, crate::opus::OpusError> {
    use crate::opus::{parse_packet, Bandwidth, Mode, OpusError};
    let mut dec = CeltDecoder::new(head.channels as usize);
    let mut out = Vec::new();
    for p in packets {
        let pk = parse_packet(p)?;
        if pk.toc.mode != Mode::Celt {
            return Err(OpusError::NotDecodedYet("SILK"));
        }
        let end_band = match pk.toc.bandwidth {
            Bandwidth::Narrow => 13,
            Bandwidth::Medium | Bandwidth::Wide => 17,
            Bandwidth::SuperWide => 19,
            Bandwidth::Full => 21,
        };
        let lm = (pk.toc.frame_units.trailing_zeros()) as usize;
        for f in &pk.frames {
            if f.len() < 2 {
                return Err(OpusError::NotDecodedYet("packet loss concealment"));
            }
            out.extend(dec.decode(f, FrameParams { stream_channels: if pk.toc.stereo { 2 } else { 1 }, end_band, lm, start: 0 }));
        }
    }
    let skip = head.pre_skip as usize * head.channels as usize;
    Ok(out.split_off(skip.min(out.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (crate::opus::Head, Vec<Vec<u8>>, Vec<i16>) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/opus");
        let bytes = std::fs::read(dir.join(format!("{name}.opus"))).unwrap();
        let raw = std::fs::read(dir.join(format!("{name}.ref.s16"))).unwrap();
        let pcm: Vec<i16> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let mut d = crate::ogg::Demuxer::new();
        let mut packets: Vec<Vec<u8>> = d.push(&bytes).into_iter().flat_map(|p| p.packets).collect();
        let head = crate::opus::parse_head(&packets[0]).unwrap();
        let audio = packets.split_off(2);
        (head, audio, pcm)
    }

    /// Signal to error ratio in dB and the correlation of the decoder's sound against the reference's.
    fn compare(got: &[f32], want: &[i16]) -> (f64, f64) {
        let n = got.len().min(want.len());
        let (mut s, mut e, mut gg, mut gw) = (0f64, 0f64, 0f64, 0f64);
        for i in 0..n {
            let (g, w) = (got[i] as f64 * 32768.0, want[i] as f64);
            s += w * w;
            e += (g - w) * (g - w);
            gg += g * g;
            gw += g * w;
        }
        (10.0 * (s / e.max(1e-9)).log10(), gw / (gg * s).sqrt().max(1e-9))
    }

    #[test]
    fn the_fast_inverse_transform_equals_the_definition_at_every_opus_size() {
        let mut x = 11u64;
        for m in [120usize, 240, 480, 960] {
            let coef: Vec<f32> = (0..m).map(|_| { x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); ((x >> 40) as f32 / 8_388_608.0) - 1.0 }).collect();
            let want = Imdct::direct(&coef);
            let mut got = vec![0.0f32; 2 * m];
            Imdct::new(2 * m).run(&coef, &mut got);
            let err = want.iter().zip(&got).map(|(a, b)| (a - *b as f64).abs()).fold(0.0, f64::max);
            assert!(err < 2e-3, "m={m}: {err}");
        }
    }

    #[test]
    #[ignore = "a speed measurement: run with --release and --ignored --nocapture"]
    fn frames_per_second_of_the_inverse_transform() {
        for m in [120usize, 480, 960] {
            let coef: Vec<f32> = (0..m).map(|i| ((i * 37) % 101) as f32 / 50.0 - 1.0).collect();
            let t = Imdct::new(2 * m);
            let mut out = vec![0.0f32; 2 * m];
            let start = std::time::Instant::now();
            let n = 2000;
            for _ in 0..n {
                t.run(&coef, &mut out);
            }
            let per = start.elapsed().as_secs_f64() / n as f64;
            eprintln!("inverse transform of {m} coefficients: {:.1} microseconds, {:.0} per second", per * 1e6, 1.0 / per);
        }
    }

    #[test]
    fn celt_fixtures_match_the_reference_decode() {
        for (name, min_snr) in [("celt-mono-20ms", 70.0), ("celt-stereo-20ms", 70.0), ("celt-mono-10ms-wb", 70.0), ("celt-stereo-5ms", 70.0), ("celt-mono-2p5ms", 70.0), ("celt-stereo-clicks", 70.0)] {
            let (head, audio, want) = fixture(name);
            let got = decode_celt_packets(&head, &audio).unwrap();
            let (snr, corr) = compare(&got, &want);
            eprintln!("{name}: {} samples, want {}, snr {snr:.1} dB, corr {corr:.5}", got.len(), want.len());
            assert!(got.len() >= want.len(), "{name}: too few samples");
            assert!(snr > min_snr && corr > 0.999, "{name}: snr {snr:.1} corr {corr:.5}");
        }
    }
}

/// Levinson-Durbin: the prediction filter `a[0..order]` (so that `x[i] + sum a[k] x[i-1-k]` is small) from autocorrelations.
fn levinson(ac: &[f64], order: usize) -> Vec<f64> {
    let mut a = vec![0.0f64; order];
    let mut err = ac[0].max(1e-9);
    for i in 0..order {
        let mut acc = ac[i + 1];
        for j in 0..i {
            acc += a[j] * ac[i - j];
        }
        let k = -acc / err;
        let prev = a.clone();
        for j in 0..i {
            a[j] = prev[j] + k * prev[i - 1 - j];
        }
        a[i] = k;
        err *= 1.0 - k * k;
        if err < 1e-12 {
            break;
        }
    }
    a
}
