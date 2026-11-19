//! The inverse modified discrete cosine transform of Vorbis: `n/2` coefficients in, `n` samples out,
//! `x[j] = sum_k X[k] cos(2 pi / n * (j + 1/2 + n/4) * (k + 1/2))`, through a DCT-IV that a complex FFT of a quarter of the size
//! computes. The direct sum is kept for the tests.

use std::f64::consts::PI;

/// Tables for one block size.
pub struct Imdct {
    n: usize,
    /// twiddles `exp(-i pi (4k + 1) / (4 m))` before the FFT of the DCT-IV of size `m = n / 2`
    tw: Vec<(f64, f64)>,
    /// twiddles `exp(-i pi k / m)` after it
    tw2: Vec<(f64, f64)>,
    /// FFT twiddles for size m/2
    fft_tw: Vec<(f64, f64)>,
    rev: Vec<usize>,
}

impl Imdct {
    pub fn new(n: usize) -> Imdct {
        assert!(n >= 8 && n.is_power_of_two());
        let m = n / 2;
        let h = m / 2;
        let tw = (0..h).map(|k| {
            let a = -PI * (4 * k + 1) as f64 / (4 * m) as f64;
            (a.cos(), a.sin())
        }).collect();
        let tw2 = (0..h).map(|k| {
            let a = -PI * k as f64 / m as f64;
            (a.cos(), a.sin())
        }).collect();
        let fft_tw = (0..h / 2).map(|k| {
            let a = -2.0 * PI * k as f64 / h as f64;
            (a.cos(), a.sin())
        }).collect();
        let bits = h.trailing_zeros();
        let rev = (0..h).map(|i| if bits == 0 { 0 } else { i.reverse_bits() >> (usize::BITS - bits) }).collect();
        Imdct { n, tw, tw2, fft_tw, rev }
    }

    fn fft(&self, re: &mut [f64], im: &mut [f64]) {
        let h = re.len();
        for i in 0..h {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= h {
            let step = h / len;
            for start in (0..h).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = self.fft_tw[k * step];
                    let (a, b) = (start + k, start + k + len / 2);
                    let (tr, ti) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }

    /// `y[j] = sum_k x[k] cos(pi / m * (j + 1/2) * (k + 1/2))`.
    fn dct4(&self, x: &[f64], y: &mut [f64]) {
        let m = x.len();
        let h = m / 2;
        let mut re = vec![0.0; h];
        let mut im = vec![0.0; h];
        for k in 0..h {
            let (a, b) = (x[2 * k], x[m - 1 - 2 * k]);
            let (c, s) = self.tw[k];
            re[k] = a * c - b * s;
            im[k] = a * s + b * c;
        }
        self.fft(&mut re, &mut im);
        for k in 0..h {
            let (c, s) = self.tw2[k];
            let (ur, ui) = (re[k] * c - im[k] * s, re[k] * s + im[k] * c);
            y[2 * k] = ur;
            y[m - 1 - 2 * k] = -ui;
        }
    }

    /// `coef` has `n / 2` values; `out` gets `n` samples.
    pub fn run(&self, coef: &[f32], out: &mut [f32]) {
        let m = self.n / 2;
        let x: Vec<f64> = coef.iter().map(|&v| v as f64).collect();
        let mut y = vec![0.0; m];
        self.dct4(&x, &mut y);
        for (j, o) in out.iter_mut().enumerate().take(self.n) {
            *o = if j < m / 2 {
                y[j + m / 2]
            } else if j < 3 * m / 2 {
                -y[3 * m / 2 - 1 - j]
            } else {
                -y[j - 3 * m / 2]
            } as f32;
        }
    }
}

/// The definition, summed directly (slow): for the tests.
pub fn direct(coef: &[f32], n: usize) -> Vec<f64> {
    (0..n).map(|j| coef.iter().enumerate().map(|(k, &c)| c as f64 * (2.0 * PI / n as f64 * (j as f64 + 0.5 + n as f64 / 4.0) * (k as f64 + 0.5)).cos()).sum()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fast_transform_equals_the_definition_at_every_block_size() {
        let mut x = 7u64;
        for n in [8usize, 16, 64, 256, 2048] {
            let coef: Vec<f32> = (0..n / 2).map(|_| { x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); ((x >> 40) as f32 / 8_388_608.0) - 1.0 }).collect();
            let want = direct(&coef, n);
            let mut got = vec![0.0f32; n];
            Imdct::new(n).run(&coef, &mut got);
            let err = want.iter().zip(&got).map(|(a, b)| (a - *b as f64).abs()).fold(0.0, f64::max);
            assert!(err < 1e-3, "n={n}: {err}");
        }
    }
}
