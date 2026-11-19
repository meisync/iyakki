//! The part of an AAC-LC decoder that needs no table of the standard (F-RADIO-12, F-RADIO-16): the ADTS frame header, the long-window
//! filterbank (sine and Kaiser-Bessel-derived windows computed from their formulas, the IMDCT of [`crate::mdct`], overlap-add), inverse
//! quantisation, the scalefactor gain and mid/side stereo.
//!
//! What is NOT here is everything that reads the Huffman codebooks of ISO/IEC 14496-3 (the scalefactor book and the eleven spectral books)
//! and the scalefactor band tables: they are long tables that cannot be written from memory without errors, and a decoder built on guessed
//! tables would play noise that looks like music. The tables enter through [`SpectrumSource`]: a source that has them (the owner supplies a
//! licensed copy, docs/development/ALPHA-7-DECISIONS.md, D-RADIO-AAC) gives the quantised values and scalefactors of each frame, and
//! [`Synth`] turns them into sound. Until then the radio plays AAC through the audio and video add-on the person downloads.

use crate::mdct::Imdct;

/// Samples per channel in one AAC frame.
pub const FRAME: usize = 1024;

/// The sample rates an ADTS header can name, by its index.
pub const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

/// What the 7 (or 9) byte ADTS header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adts {
    /// 1 Main, 2 LC, 3 SSR, 4 LTP (the ADTS profile field plus one).
    pub object_type: u8,
    pub rate: u32,
    pub channels: u8,
    /// The whole frame, header included.
    pub frame_len: usize,
    pub header_len: usize,
    pub has_crc: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AdtsError {
    Short,
    NoSync,
    BadRate,
    BadLength,
}

impl Adts {
    pub fn parse(b: &[u8]) -> Result<Adts, AdtsError> {
        if b.len() < 7 {
            return Err(AdtsError::Short);
        }
        if b[0] != 0xFF || b[1] & 0xF6 != 0xF0 {
            // sync word 0xFFF, layer 00
            return Err(AdtsError::NoSync);
        }
        let has_crc = b[1] & 1 == 0;
        let object_type = ((b[2] >> 6) & 3) + 1;
        let idx = ((b[2] >> 2) & 15) as usize;
        let rate = *RATES.get(idx).ok_or(AdtsError::BadRate)?;
        let channels = (((b[2] & 1) << 2) | (b[3] >> 6)) & 7;
        let frame_len = (((b[3] & 3) as usize) << 11) | ((b[4] as usize) << 3) | ((b[5] >> 5) as usize);
        let header_len = if has_crc { 9 } else { 7 };
        if frame_len < header_len {
            return Err(AdtsError::BadLength);
        }
        Ok(Adts { object_type, rate, channels, frame_len, header_len, has_crc })
    }
}

/// A stream cut into ADTS frames: the offsets of every whole frame in `data` (a damaged part is skipped by looking for the next sync).
pub fn frames_of(data: &[u8]) -> Vec<(usize, Adts)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 7 <= data.len() && out.len() < 1_000_000 {
        match Adts::parse(&data[i..]) {
            Ok(h) if i + h.frame_len <= data.len() => {
                let next = i + h.frame_len;
                // a header is believed when the next one follows it (or the data ends): a chance sync word inside a frame is not a frame
                if next == data.len() || (next + 7 <= data.len() && Adts::parse(&data[next..]).is_ok()) {
                    out.push((i, h));
                    i = next;
                    continue;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// `sign(q) * |q|^(4/3)`, the inverse quantiser of a spectral value.
pub fn inverse_quant(q: i32) -> f32 {
    let m = (q.unsigned_abs() as f64).powf(4.0 / 3.0) as f32;
    if q < 0 { -m } else { m }
}

/// The gain a scalefactor gives its band: `2^((sf - 100) / 4)`.
pub fn scalefactor_gain(sf: i32) -> f32 {
    (2f64).powf((sf - 100) as f64 / 4.0) as f32
}

/// The sine window of a long block (`2 * FRAME` points).
pub fn sine_window() -> Vec<f32> {
    let n = 2 * FRAME;
    (0..n).map(|i| ((std::f64::consts::PI / n as f64) * (i as f64 + 0.5)).sin() as f32).collect()
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0f64, 1.0f64, 1.0f64);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)).powi(2);
        sum += term;
        k += 1.0;
    }
    sum
}

/// The Kaiser-Bessel-derived window of a long block (`alpha` 4 for long blocks): power complementary by construction.
pub fn kbd_window(alpha: f64) -> Vec<f32> {
    let n = 2 * FRAME;
    let kaiser: Vec<f64> = (0..=FRAME).map(|i| bessel_i0(std::f64::consts::PI * alpha * (1.0 - (2.0 * i as f64 / FRAME as f64 - 1.0).powi(2)).max(0.0).sqrt())).collect();
    let total: f64 = kaiser.iter().sum();
    let mut w = vec![0f32; n];
    let mut acc = 0.0;
    for i in 0..FRAME {
        acc += kaiser[i];
        w[i] = (acc / total).sqrt() as f32;
        w[n - 1 - i] = w[i];
    }
    w
}

/// Mid/side to left/right in place, for the bands the stream marks (`used[i]` for coefficient `i`): `L = M + S`, `R = M - S`.
pub fn mid_side(left: &mut [f32], right: &mut [f32], used: &[bool]) {
    for i in 0..left.len().min(right.len()).min(used.len()) {
        if used[i] {
            let (m, s) = (left[i], right[i]);
            left[i] = m + s;
            right[i] = m - s;
        }
    }
}

/// What the part that reads the codebooks hands to the filterbank, for one channel of one frame.
pub struct ChannelSpectrum {
    /// The quantised value of each of the 1024 lines.
    pub quant: Vec<i32>,
    /// The scalefactor of the band each line belongs to.
    pub sf: Vec<i32>,
}

/// The table-driven half: it knows the codebooks and the band tables. `None` ends the stream (or means it is damaged: the frame is silence).
pub trait SpectrumSource {
    fn next_frame(&mut self) -> Option<Vec<ChannelSpectrum>>;
}

/// Long-block synthesis with overlap-add, for one or two channels. Short blocks and window switching come with the tables.
pub struct Synth {
    imdct: Imdct,
    window: Vec<f32>,
    tail: Vec<Vec<f32>>,
}

impl Synth {
    pub fn new(channels: usize, kbd: bool) -> Synth {
        Synth { imdct: Imdct::new(2 * FRAME), window: if kbd { kbd_window(4.0) } else { sine_window() }, tail: vec![vec![0.0; FRAME]; channels.max(1)] }
    }

    /// One frame of one channel: the spectrum in, 1024 samples out (as floats, full scale 32768).
    pub fn frame(&mut self, ch: usize, s: &ChannelSpectrum) -> Vec<f32> {
        let mut spec = vec![0f32; FRAME];
        for k in 0..FRAME.min(s.quant.len()).min(s.sf.len()) {
            spec[k] = inverse_quant(s.quant[k]) * scalefactor_gain(s.sf[k]);
        }
        let mut y = vec![0f32; 2 * FRAME];
        self.imdct.run(&spec, &mut y);
        // the transform's own scale, then the window; the first half joins the second half of the frame before
        let scale = 2.0 / (2 * FRAME) as f32;
        let idx = ch.min(self.tail.len() - 1);
        let tail = &mut self.tail[idx];
        let mut out = vec![0f32; FRAME];
        for i in 0..FRAME {
            out[i] = tail[i] + y[i] * self.window[i] * scale;
            tail[i] = y[FRAME + i] * self.window[FRAME + i] * scale;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(profile: u8, idx: u8, chans: u8, frame_len: usize, crc: bool) -> [u8; 7] {
        let mut h = [0xFF, 0xF0 | if crc { 0 } else { 1 }, 0, 0, 0, 0, 0xFC];
        h[2] = (profile << 6) | (idx << 2) | (chans >> 2);
        h[3] = ((chans & 3) << 6) | ((frame_len >> 11) & 3) as u8;
        h[4] = ((frame_len >> 3) & 0xFF) as u8;
        h[5] = (((frame_len & 7) << 5) | 0x1F) as u8;
        h
    }

    #[test]
    fn an_adts_header_gives_profile_rate_channels_and_length() {
        let h = Adts::parse(&header(1, 4, 2, 371, false)).unwrap();
        assert_eq!((h.object_type, h.rate, h.channels, h.frame_len, h.header_len), (2, 44100, 2, 371, 7));
        let c = Adts::parse(&header(1, 3, 1, 200, true)).unwrap();
        assert_eq!((c.rate, c.channels, c.header_len, c.has_crc), (48000, 1, 9, true));
        assert_eq!(Adts::parse(&[0xFF, 0xF1, 0]), Err(AdtsError::Short));
        assert_eq!(Adts::parse(&[0x12; 7]), Err(AdtsError::NoSync));
        assert_eq!(Adts::parse(&header(1, 14, 2, 100, false)), Err(AdtsError::BadRate));
        assert_eq!(Adts::parse(&header(1, 4, 2, 3, false)), Err(AdtsError::BadLength));
    }

    #[test]
    fn a_stream_is_cut_into_frames_and_a_chance_sync_word_inside_one_is_not_a_frame() {
        let mut data = Vec::new();
        for n in 0..4 {
            let len = 40 + n * 3;
            data.extend_from_slice(&header(1, 4, 2, len, false));
            data.extend(std::iter::repeat(0x11).take(len - 7));
        }
        // a sync-looking pair inside the second frame's payload
        data[60] = 0xFF;
        data[61] = 0xF1;
        let f = frames_of(&data);
        assert_eq!(f.len(), 4);
        assert_eq!(f.iter().map(|(o, _)| *o).collect::<Vec<_>>(), vec![0, 40, 83, 129]);
        // junk in front and a cut tail are skipped
        let mut junk = vec![0u8; 13];
        junk.extend_from_slice(&data[..100]);
        assert_eq!(frames_of(&junk).len(), 2);
    }

    #[test]
    fn the_quantiser_and_the_gain_follow_their_formulas() {
        assert_eq!(inverse_quant(0), 0.0);
        assert!((inverse_quant(8) - 16.0).abs() < 1e-4, "8^(4/3) = 16");
        assert!((inverse_quant(-27) + 81.0).abs() < 1e-3, "27^(4/3) = 81");
        assert!((scalefactor_gain(100) - 1.0).abs() < 1e-6 && (scalefactor_gain(104) - 2.0).abs() < 1e-5 && (scalefactor_gain(96) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn both_windows_are_power_complementary_so_the_overlap_adds_back_to_the_signal() {
        for w in [sine_window(), kbd_window(4.0)] {
            assert_eq!(w.len(), 2 * FRAME);
            for i in 0..FRAME {
                let s = w[i] * w[i] + w[i + FRAME] * w[i + FRAME];
                assert!((s - 1.0).abs() < 1e-4, "{i}: {s}");
                assert!((w[i] - w[2 * FRAME - 1 - i]).abs() < 1e-6, "symmetric");
            }
        }
    }

    #[test]
    fn mid_side_gives_left_and_right_only_where_the_stream_says() {
        let (mut l, mut r) = (vec![3.0, 3.0], vec![1.0, 1.0]);
        mid_side(&mut l, &mut r, &[true, false]);
        assert_eq!((l, r), (vec![4.0, 3.0], vec![2.0, 1.0]));
    }

    /// Goertzel: how much of `freq_bin` (cycles per 2048 samples) a block holds.
    fn power_at(x: &[f32], bin: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * bin / x.len() as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            re += *v as f64 * (w * i as f64).cos();
            im -= *v as f64 * (w * i as f64).sin();
        }
        re * re + im * im
    }

    #[test]
    fn a_spectrum_with_one_line_comes_out_as_a_steady_tone_of_that_frequency_frame_after_frame() {
        struct One(u32);
        impl SpectrumSource for One {
            fn next_frame(&mut self) -> Option<Vec<ChannelSpectrum>> {
                if self.0 == 0 {
                    return None;
                }
                self.0 -= 1;
                let mut quant = vec![0; FRAME];
                quant[64] = 100;
                Some(vec![ChannelSpectrum { quant, sf: vec![100; FRAME] }])
            }
        }
        let mut src = One(6);
        let mut synth = Synth::new(1, false);
        let mut all: Vec<f32> = Vec::new();
        while let Some(f) = src.next_frame() {
            all.extend(synth.frame(0, &f[0]));
        }
        assert_eq!(all.len(), 6 * FRAME);
        // from the second frame on the block is steady: line 64 of 1024 is bin 64 of a 2048 block (the transform has 2N points), a clear peak
        let block = &all[2 * FRAME..4 * FRAME];
        let peak = power_at(block, 64.5);
        let off = power_at(block, 40.5).max(power_at(block, 90.5));
        assert!(peak > 1000.0 * off.max(1e-9), "peak {peak} against {off}");
        let amp = block.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        assert!(amp > 0.1 && amp < 2.0 && amp.is_finite(), "{amp}");
        // silence in, silence out
        let mut synth = Synth::new(2, true);
        let z = ChannelSpectrum { quant: vec![0; FRAME], sf: vec![100; FRAME] };
        assert!(synth.frame(1, &z).iter().all(|v| *v == 0.0));
    }
}
