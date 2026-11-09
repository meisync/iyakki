//! The audio front end: 16 kHz mono samples to log-mel frames (25 ms frames every 10 ms, pre-emphasis, Hann window, 512-point FFT,
//! 40 triangular mel filters) and the per-window normalisation. Pure functions of the samples, no files, no clocks.

use crate::fft::Fft;
use std::f32::consts::PI;

pub const SAMPLE_RATE: usize = 16_000;
/// 25 ms.
pub const FRAME: usize = 400;
/// 10 ms.
pub const HOP: usize = 160;
pub const NFFT: usize = 512;
pub const N_MEL: usize = 40;
/// One model window: 148 frames, 400 + 147 * 160 = 23 920 samples, about 1.5 s.
pub const WINDOW_FRAMES: usize = 148;
pub const WINDOW_SAMPLES: usize = FRAME + (WINDOW_FRAMES - 1) * HOP;
const PRE_EMPHASIS: f32 = 0.97;
const LOW_HZ: f32 = 60.0;
const HIGH_HZ: f32 = 7800.0;
/// The floor under the log so silence is a finite number.
pub const LOG_FLOOR: f32 = 1e-10;
/// Per-window scale never goes below this (in natural-log units), so a silent window is not blown up into structure.
pub const MIN_SCALE: f32 = 0.5;

pub fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

pub fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10f32.powf(mel / 2595.0) - 1.0)
}

/// The periodic Hann window of length `n`.
pub fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos()).collect()
}

/// The mel filterbank: `N_MEL` rows of `NFFT / 2 + 1` weights (triangles, peak 1.0 at the centre bin position).
pub fn mel_filterbank() -> Vec<Vec<f32>> {
    let bins = NFFT / 2 + 1;
    let (lo, hi) = (hz_to_mel(LOW_HZ), hz_to_mel(HIGH_HZ));
    let pts: Vec<f32> = (0..N_MEL + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f32 / (N_MEL + 1) as f32)).collect();
    let bin_hz = SAMPLE_RATE as f32 / NFFT as f32;
    (0..N_MEL)
        .map(|m| {
            let (a, b, c) = (pts[m], pts[m + 1], pts[m + 2]);
            (0..bins)
                .map(|k| {
                    let f = k as f32 * bin_hz;
                    if f <= a || f >= c {
                        0.0
                    } else if f <= b {
                        (f - a) / (b - a)
                    } else {
                        (c - f) / (c - b)
                    }
                })
                .collect()
        })
        .collect()
}

/// Turns frames of samples into log-mel vectors. Holds the tables only.
pub struct LogMel {
    fft: Fft,
    window: Vec<f32>,
    bank: Vec<Vec<f32>>,
}

impl Default for LogMel {
    fn default() -> Self {
        Self::new()
    }
}

impl LogMel {
    pub fn new() -> LogMel {
        LogMel { fft: Fft::new(NFFT), window: hann(FRAME), bank: mel_filterbank() }
    }

    /// The power spectrum (`NFFT / 2 + 1` bins) of one frame of `FRAME` samples, after pre-emphasis and the window.
    /// `prev` is the sample before the frame (0.0 at the start of a stream).
    pub fn power(&self, frame: &[f32], prev: f32) -> Vec<f32> {
        debug_assert_eq!(frame.len(), FRAME);
        let mut re = vec![0.0f32; NFFT];
        let mut im = vec![0.0f32; NFFT];
        let mut last = prev;
        for i in 0..FRAME {
            re[i] = (frame[i] - PRE_EMPHASIS * last) * self.window[i];
            last = frame[i];
        }
        self.fft.forward(&mut re, &mut im);
        (0..=NFFT / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect()
    }

    /// One log-mel vector from a frame.
    pub fn frame(&self, frame: &[f32], prev: f32) -> [f32; N_MEL] {
        let p = self.power(frame, prev);
        let mut out = [0.0f32; N_MEL];
        for (m, row) in self.bank.iter().enumerate() {
            let e: f32 = row.iter().zip(&p).map(|(w, x)| w * x).sum();
            out[m] = e.max(LOG_FLOOR).ln();
        }
        out
    }

    /// All the frames of a clip, row-major `[frame][mel]`. A clip shorter than one frame gives none.
    pub fn clip(&self, samples: &[f32]) -> Vec<f32> {
        if samples.len() < FRAME {
            return Vec::new();
        }
        let n = (samples.len() - FRAME) / HOP + 1;
        let mut out = Vec::with_capacity(n * N_MEL);
        for t in 0..n {
            let s = t * HOP;
            let prev = if s == 0 { 0.0 } else { samples[s - 1] };
            out.extend_from_slice(&self.frame(&samples[s..s + FRAME], prev));
        }
        out
    }
}

/// How many frames `n` samples give.
pub fn frames_for(n: usize) -> usize {
    if n < FRAME { 0 } else { (n - FRAME) / HOP + 1 }
}

/// Per-window normalisation, in place on `[frame][mel]`: each mel bin loses its mean over the window (the room's steady colour and the
/// microphone's gain drop out), then everything is divided by one scale, the standard deviation of the whole window but never below
/// `MIN_SCALE`.
pub fn normalise_window(x: &mut [f32]) {
    let t = x.len() / N_MEL;
    if t == 0 {
        return;
    }
    for m in 0..N_MEL {
        let mean: f32 = (0..t).map(|i| x[i * N_MEL + m]).sum::<f32>() / t as f32;
        for i in 0..t {
            x[i * N_MEL + m] -= mean;
        }
    }
    let var: f32 = x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32;
    let scale = var.sqrt().max(MIN_SCALE);
    for v in x.iter_mut() {
        *v /= scale;
    }
}

/// A clip as one normalised model window: the clip is cut or padded with silence (at the end) to `WINDOW_FRAMES` frames.
pub fn window_of(lm: &LogMel, samples: &[f32]) -> Vec<f32> {
    let mut s = samples.to_vec();
    s.resize(WINDOW_SAMPLES, 0.0);
    let mut f = lm.clip(&s);
    f.truncate(WINDOW_FRAMES * N_MEL);
    normalise_window(&mut f);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mel_scale_is_the_htk_one() {
        assert!((hz_to_mel(1000.0) - 1000.0).abs() < 0.1, "1000 Hz is 1000 mel");
        assert!(hz_to_mel(0.0).abs() < 1e-6);
        assert!((mel_to_hz(hz_to_mel(3210.0)) - 3210.0).abs() < 0.05);
    }

    #[test]
    fn the_hann_window_has_its_hand_computed_values() {
        let w = hann(8);
        let expect = [0.0, 0.14644660, 0.5, 0.85355339, 1.0, 0.85355339, 0.5, 0.14644660];
        for (a, b) in w.iter().zip(expect) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn framing_counts_follow_the_25_and_10_ms_rule() {
        assert_eq!(frames_for(399), 0);
        assert_eq!(frames_for(400), 1);
        assert_eq!(frames_for(559), 1);
        assert_eq!(frames_for(560), 2);
        assert_eq!(frames_for(16_000), 98);
        assert_eq!(frames_for(WINDOW_SAMPLES), WINDOW_FRAMES);
        let lm = LogMel::new();
        assert_eq!(lm.clip(&vec![0.1; 16_000]).len(), 98 * N_MEL);
        assert!(lm.clip(&[0.0; 100]).is_empty());
    }

    #[test]
    fn the_filterbank_is_triangles_that_rise_to_one_and_grow_wider() {
        let b = mel_filterbank();
        assert_eq!(b.len(), N_MEL);
        let width = |r: &Vec<f32>| r.iter().filter(|v| **v > 0.0).count();
        for r in &b {
            let peak = r.iter().cloned().fold(0.0, f32::max);
            assert!(peak > 0.5 && peak <= 1.0 + 1e-6);
            let first = r.iter().position(|v| *v > 0.0).unwrap();
            let last = r.iter().rposition(|v| *v > 0.0).unwrap();
            assert!(r[first..=last].iter().all(|v| *v > 0.0), "no holes");
        }
        assert!(width(&b[N_MEL - 1]) > width(&b[0]) * 3, "high filters are wider in Hz");
        // overlapping neighbours: the bank sums to about 1 over the middle of the range
        let bin = 100;
        let s: f32 = b.iter().map(|r| r[bin]).sum();
        assert!((s - 1.0).abs() < 0.2, "sum {s}");
    }

    #[test]
    fn a_1_khz_tone_lights_the_mel_bin_that_holds_1_khz() {
        let lm = LogMel::new();
        let x: Vec<f32> = (0..FRAME).map(|t| 0.5 * (2.0 * PI * 1000.0 * t as f32 / SAMPLE_RATE as f32).sin()).collect();
        let v = lm.frame(&x, 0.0);
        let top = v.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        let bank = mel_filterbank();
        let k = (1000.0 / (SAMPLE_RATE as f32 / NFFT as f32)).round() as usize;
        assert!(bank[top][k] > 0.5, "bin {top} must cover 1 kHz");
        assert!(v[top] > v[0] + 3.0 && v[top] > v[N_MEL - 1] + 3.0);
    }

    #[test]
    fn silence_gives_the_floor_and_a_constant_has_a_known_dc_power() {
        let lm = LogMel::new();
        let v = lm.frame(&[0.0; FRAME], 0.0);
        assert!(v.iter().all(|x| (*x - LOG_FLOOR.ln()).abs() < 1e-3));
        // DC input c, no pre-emphasis history: x[0] = c, then c*(1-0.97); power at bin 0 equals (sum of windowed samples)^2
        let p = lm.power(&[0.5; FRAME], 0.5);
        let w = hann(FRAME);
        let sum: f32 = w.iter().map(|v| v * 0.5 * (1.0 - PRE_EMPHASIS)).sum();
        assert!((p[0] - sum * sum).abs() < 1e-3 * sum * sum.max(1.0), "{} vs {}", p[0], sum * sum);
    }

    #[test]
    fn normalising_removes_the_bin_means_and_never_amplifies_silence() {
        let mut x: Vec<f32> = (0..WINDOW_FRAMES * N_MEL).map(|i| 3.0 + (i % N_MEL) as f32 + if i / N_MEL == 5 { 2.0 } else { 0.0 }).collect();
        normalise_window(&mut x);
        for m in 0..N_MEL {
            let mean: f32 = (0..WINDOW_FRAMES).map(|t| x[t * N_MEL + m]).sum::<f32>() / WINDOW_FRAMES as f32;
            assert!(mean.abs() < 1e-4);
        }
        let mut s = vec![-23.0f32; WINDOW_FRAMES * N_MEL];
        for (i, v) in s.iter_mut().enumerate() {
            *v += if i % 7 == 0 { 0.01 } else { 0.0 };
        }
        normalise_window(&mut s);
        assert!(s.iter().all(|v| v.abs() < 0.1), "a silent window stays small");
    }

    #[test]
    fn a_short_clip_is_padded_to_a_full_window() {
        let lm = LogMel::new();
        let w = window_of(&lm, &vec![0.2; 8000]);
        assert_eq!(w.len(), WINDOW_FRAMES * N_MEL);
    }
}
