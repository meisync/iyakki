//! Data augmentation on 16 kHz mono f32: gain, time shift (placing a clip in a window), additive noise at a random SNR, a synthetic
//! exponential-decay room impulse response, and speed 0.9 to 1.1. Everything draws from a seeded `Rng`.

use crate::fft::Fft;
use crate::features::{SAMPLE_RATE, WINDOW_SAMPLES};
use crate::rng::Rng;

pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() { 0.0 } else { (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt() }
}

/// Multiply by a gain given in decibels.
pub fn gain(x: &mut [f32], db: f32) {
    let g = 10f32.powf(db / 20.0);
    x.iter_mut().for_each(|v| *v *= g);
}

/// Speed change by resampling (pitch moves with it, as a speaking-rate change does for a quick augmentation). `factor` 1.1 is faster.
pub fn speed(x: &[f32], factor: f32) -> Vec<f32> {
    if x.is_empty() || (factor - 1.0).abs() < 1e-4 {
        return x.to_vec();
    }
    let n = (x.len() as f32 / factor) as usize;
    (0..n)
        .map(|i| {
            let pos = i as f32 * factor;
            let a = pos as usize;
            let f = pos - a as f32;
            let b = (a + 1).min(x.len() - 1);
            x[a.min(x.len() - 1)] * (1.0 - f) + x[b] * f
        })
        .collect()
}

/// Mix `noise` into `x` so that the speech RMS over the noise RMS is `snr_db`. The noise is looped from a random start.
pub fn add_noise(x: &mut [f32], noise: &[f32], snr_db: f32, rng: &mut Rng) {
    if noise.is_empty() || x.is_empty() {
        return;
    }
    let start = rng.below(noise.len());
    let n: Vec<f32> = (0..x.len()).map(|i| noise[(start + i) % noise.len()]).collect();
    let (sr, nr) = (rms(x), rms(&n));
    if nr < 1e-9 {
        return;
    }
    // when the clip is mostly silence its RMS says little; measure the speech against the loud part
    let target = sr.max(1e-4) / 10f32.powf(snr_db / 20.0);
    let g = target / nr;
    for (v, w) in x.iter_mut().zip(n) {
        *v += g * w;
    }
}

/// A synthetic room impulse response: the direct sound, then noise whose level falls exponentially (60 dB in `rt60` seconds).
pub fn synthetic_rir(rt60: f32, rng: &mut Rng) -> Vec<f32> {
    let len = ((rt60.clamp(0.05, 0.8) * SAMPLE_RATE as f32) as usize).max(32);
    let mut h = vec![0.0f32; len];
    h[0] = 1.0;
    let k = 6.9078 / (rt60.max(0.05) * SAMPLE_RATE as f32);
    let early = 40 + rng.below(400); // the first reflections arrive a few ms later
    for (i, v) in h.iter_mut().enumerate().skip(early.min(len - 1)) {
        *v += 0.35 * rng.normal() * (-(k * (i - early.min(len - 1)) as f32)).exp();
    }
    let e: f32 = h.iter().map(|v| v * v).sum::<f32>().sqrt();
    h.iter_mut().for_each(|v| *v /= e);
    h
}

/// Linear convolution by FFT, truncated to the length of `x`.
pub fn convolve(x: &[f32], h: &[f32]) -> Vec<f32> {
    if x.is_empty() || h.is_empty() {
        return x.to_vec();
    }
    let n = (x.len() + h.len()).next_power_of_two().max(2);
    let fft = Fft::new(n);
    let (mut ar, mut ai) = (vec![0.0f32; n], vec![0.0f32; n]);
    let (mut br, mut bi) = (vec![0.0f32; n], vec![0.0f32; n]);
    ar[..x.len()].copy_from_slice(x);
    br[..h.len()].copy_from_slice(h);
    fft.forward(&mut ar, &mut ai);
    fft.forward(&mut br, &mut bi);
    let (mut cr, mut ci) = (vec![0.0f32; n], vec![0.0f32; n]);
    // multiply, then inverse via the conjugate trick
    for k in 0..n {
        let (r, i) = (ar[k] * br[k] - ai[k] * bi[k], ar[k] * bi[k] + ai[k] * br[k]);
        cr[k] = r;
        ci[k] = -i;
    }
    fft.forward(&mut cr, &mut ci);
    (0..x.len()).map(|t| cr[t] / n as f32).collect()
}

/// Trim leading and trailing silence (below `floor_db` relative to the peak of a 10 ms RMS track) and keep `margin` samples.
pub fn trim_silence(x: &[f32], floor_db: f32, margin: usize) -> Vec<f32> {
    let hop = 160;
    let env: Vec<f32> = x.chunks(hop).map(rms).collect();
    let peak = env.iter().cloned().fold(0.0, f32::max);
    if peak < 1e-6 {
        return x.to_vec();
    }
    let thr = peak * 10f32.powf(floor_db / 20.0);
    let first = env.iter().position(|v| *v >= thr).unwrap_or(0);
    let last = env.iter().rposition(|v| *v >= thr).unwrap_or(env.len() - 1);
    let a = (first * hop).saturating_sub(margin);
    let b = ((last + 1) * hop + margin).min(x.len());
    x[a..b].to_vec()
}

/// Some background to mix in when no recorded noise is supplied: white, pink, a hum with harmonics, or a moving "fan" band.
pub fn synthetic_noise(kind: usize, len: usize, rng: &mut Rng) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    match kind % 4 {
        0 => out.iter_mut().for_each(|v| *v = rng.normal() * 0.1),
        1 => {
            // pink-ish: three one-pole filtered white noises
            let (mut a, mut b, mut c) = (0.0f32, 0.0f32, 0.0f32);
            for v in out.iter_mut() {
                let w = rng.normal();
                a = 0.99765 * a + w * 0.0990460;
                b = 0.96300 * b + w * 0.2965164;
                c = 0.57000 * c + w * 1.0526913;
                *v = (a + b + c + w * 0.1848) * 0.05;
            }
        }
        2 => {
            let f0 = rng.range(45.0, 130.0);
            for (i, v) in out.iter_mut().enumerate() {
                let t = i as f32 / SAMPLE_RATE as f32;
                *v = (1..=5).map(|h| (2.0 * std::f32::consts::PI * f0 * h as f32 * t).sin() / h as f32).sum::<f32>() * 0.08 + rng.normal() * 0.01;
            }
        }
        _ => {
            let mut lp = 0.0f32;
            let a = rng.range(0.6, 0.95);
            for v in out.iter_mut() {
                lp = a * lp + (1.0 - a) * rng.normal();
                *v = lp * 0.3;
            }
        }
    }
    out
}

/// Random settings for one training example, kept so tests can pin them.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub gain_db: f32,
    pub speed: f32,
    pub snr_db: f32,
    pub rt60: Option<f32>,
}

impl Recipe {
    pub fn random(rng: &mut Rng) -> Recipe {
        Recipe {
            gain_db: rng.range(-18.0, 3.0),
            speed: rng.range(0.9, 1.1),
            snr_db: rng.range(0.0, 25.0),
            rt60: if rng.chance(0.35) { Some(rng.range(0.1, 0.6)) } else { None },
        }
    }
}

/// Put a clip at a random place in a silent model window (the whole clip inside when it fits).
pub fn place_in_window(clip: &[f32], rng: &mut Rng) -> (Vec<f32>, usize) {
    let mut w = vec![0.0f32; WINDOW_SAMPLES];
    if clip.len() >= WINDOW_SAMPLES {
        let s = rng.below(clip.len() - WINDOW_SAMPLES + 1);
        w.copy_from_slice(&clip[s..s + WINDOW_SAMPLES]);
        return (w, 0);
    }
    let off = rng.below(WINDOW_SAMPLES - clip.len() + 1);
    w[off..off + clip.len()].copy_from_slice(clip);
    (w, off)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.5 * (i as f32 * 0.1).sin()).collect()
    }

    #[test]
    fn gain_in_decibels_is_exact() {
        let mut x = vec![0.5f32; 4];
        gain(&mut x, 6.0206);
        assert!((x[0] - 1.0).abs() < 1e-3);
        gain(&mut x, -20.0);
        assert!((x[0] - 0.1).abs() < 1e-3);
    }

    #[test]
    fn speed_changes_the_length_by_the_factor() {
        let x = tone(16_000);
        assert_eq!(speed(&x, 1.0).len(), 16_000);
        assert_eq!(speed(&x, 1.25).len(), 12_800);
        assert!((speed(&x, 0.9).len() as i32 - 17_777).abs() <= 1);
    }

    #[test]
    fn noise_is_mixed_at_the_asked_snr() {
        let mut r = Rng::new(3);
        let x = tone(8000);
        let noise: Vec<f32> = (0..5000).map(|_| r.normal()).collect();
        for snr in [0.0f32, 10.0, 20.0] {
            let mut y = x.clone();
            add_noise(&mut y, &noise, snr, &mut r);
            let n: Vec<f32> = y.iter().zip(&x).map(|(a, b)| a - b).collect();
            let got = 20.0 * (rms(&x) / rms(&n)).log10();
            assert!((got - snr).abs() < 0.2, "asked {snr}, got {got}");
        }
    }

    #[test]
    fn the_rir_decays_exponentially_and_is_normalised() {
        let mut r = Rng::new(5);
        let h = synthetic_rir(0.4, &mut r);
        assert_eq!(h.len(), 6400);
        assert!((h.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-4);
        let seg = |a: usize, b: usize| h[a..b].iter().map(|v| v * v).sum::<f32>();
        assert!(seg(500, 1500) > seg(3000, 4000) && seg(3000, 4000) > seg(5400, 6400), "energy falls with time");
        // 60 dB in rt60: the last tenth is far below the early part
        assert!(seg(5760, 6400) < seg(500, 1140) * 0.02);
    }

    #[test]
    fn convolution_with_a_delayed_impulse_delays_and_matches_the_direct_sum() {
        let x = tone(300);
        let mut h = vec![0.0; 10];
        h[3] = 1.0;
        let y = convolve(&x, &h);
        assert_eq!(y.len(), 300);
        for t in 3..300 {
            assert!((y[t] - x[t - 3]).abs() < 1e-4, "t {t}");
        }
        let h2 = [0.5f32, 0.25, -0.125];
        let y2 = convolve(&x, &h2);
        for t in 2..300 {
            let d = 0.5 * x[t] + 0.25 * x[t - 1] - 0.125 * x[t - 2];
            assert!((y2[t] - d).abs() < 1e-4);
        }
    }

    #[test]
    fn trimming_keeps_the_speech_and_a_margin() {
        let mut x = vec![0.0f32; 16_000];
        for (i, v) in x[4000..8000].iter_mut().enumerate() {
            *v = 0.4 * (i as f32 * 0.2).sin();
        }
        let t = trim_silence(&x, -40.0, 320);
        assert!(t.len() >= 4000 && t.len() <= 4000 + 2 * 320 + 160, "{}", t.len());
    }

    #[test]
    fn placing_puts_the_whole_clip_inside_the_window() {
        let mut r = Rng::new(8);
        let clip = vec![1.0f32; 12_000];
        for _ in 0..20 {
            let (w, off) = place_in_window(&clip, &mut r);
            assert_eq!(w.len(), WINDOW_SAMPLES);
            assert!(w[off..off + 12_000].iter().all(|v| *v == 1.0));
            assert_eq!(w.iter().filter(|v| **v == 1.0).count(), 12_000);
        }
        let long = vec![0.5f32; WINDOW_SAMPLES + 500];
        assert_eq!(place_in_window(&long, &mut r).0.len(), WINDOW_SAMPLES);
    }

    #[test]
    fn recipes_stay_in_their_ranges_and_repeat_with_the_seed() {
        let (mut a, mut b) = (Rng::new(2), Rng::new(2));
        for _ in 0..200 {
            let r = Recipe::random(&mut a);
            let r2 = Recipe::random(&mut b);
            assert_eq!(r.speed, r2.speed);
            assert!((0.9..1.1).contains(&r.speed) && (0.0..25.0).contains(&r.snr_db) && (-18.0..3.0).contains(&r.gain_db));
        }
    }
}
