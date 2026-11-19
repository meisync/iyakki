//! An in-house radix-2 FFT (iterative, in place, f32). Sizes are powers of two.

use std::f32::consts::PI;

/// The twiddle factors and the bit-reversal table of one size, built once.
pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
}

impl Fft {
    /// `n` must be a power of two (at least 2).
    pub fn new(n: usize) -> Fft {
        assert!(n >= 2 && n.is_power_of_two(), "FFT size must be a power of two");
        let bits = n.trailing_zeros();
        let rev = (0..n).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        let cos = (0..n / 2).map(|k| (-2.0 * PI * k as f32 / n as f32).cos()).collect();
        let sin = (0..n / 2).map(|k| (-2.0 * PI * k as f32 / n as f32).sin()).collect();
        Fft { n, cos, sin, rev }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Forward transform in place: `re[k] + i im[k] = sum x[t] e^(-2 pi i k t / n)`.
    pub fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        assert!(re.len() == self.n && im.len() == self.n);
        for i in 0..self.n {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= self.n {
            let half = size / 2;
            let step = self.n / size;
            let mut start = 0;
            while start < self.n {
                for k in 0..half {
                    let (c, s) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * c - im[b] * s;
                    let ti = re[b] * s + im[b] * c;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
                start += size;
            }
            size *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_impulse_is_flat_and_a_constant_is_one_bin() {
        let f = Fft::new(8);
        let (mut re, mut im) = ([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], [0.0; 8]);
        f.forward(&mut re, &mut im);
        assert!(re.iter().all(|v| (v - 1.0).abs() < 1e-6) && im.iter().all(|v| v.abs() < 1e-6));
        let (mut re, mut im) = ([1.0f32; 8], [0.0f32; 8]);
        f.forward(&mut re, &mut im);
        assert!((re[0] - 8.0).abs() < 1e-5);
        assert!(re[1..].iter().chain(im.iter()).all(|v| v.abs() < 1e-5));
    }

    #[test]
    fn a_cosine_lands_in_its_bin_and_matches_the_naive_dft() {
        let n = 64;
        let x: Vec<f32> = (0..n).map(|t| (2.0 * PI * 5.0 * t as f32 / n as f32).cos() + 0.3 * (t as f32 * 0.37).sin()).collect();
        let f = Fft::new(n);
        let (mut re, mut im) = (x.clone(), vec![0.0; n]);
        f.forward(&mut re, &mut im);
        for k in 0..n {
            let (mut sr, mut si) = (0.0f64, 0.0f64);
            for (t, v) in x.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * t) as f64 / n as f64;
                sr += *v as f64 * a.cos();
                si += *v as f64 * a.sin();
            }
            assert!((re[k] as f64 - sr).abs() < 1e-3 && (im[k] as f64 - si).abs() < 1e-3, "bin {k}");
        }
        let pure: Vec<f32> = (0..n).map(|t| (2.0 * PI * 5.0 * t as f32 / n as f32).cos()).collect();
        let (mut re, mut im) = (pure, vec![0.0; n]);
        f.forward(&mut re, &mut im);
        assert!((re[5] - 32.0).abs() < 1e-3 && (re[n - 5] - 32.0).abs() < 1e-3 && re[6].abs() < 1e-3);
    }
}
