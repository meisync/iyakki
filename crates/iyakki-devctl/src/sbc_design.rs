//! Designs the SBC filter-bank window tables from first principles (see docs/development/SBC-TABLES.md).
//!
//! The SBC filter banks are cosine-modulated banks of `M` = 4 or 8 bands whose prototype lowpass filter has `10 M` taps, is symmetric
//! (`h[n] = h[10 M - n]`) and has `h[0] = 0`. Such a bank reconstructs the input nearly perfectly when the prototype is a "pseudo-QMF" prototype
//! (Nguyen and Koilpillai's condition, textbook material since the early 1990s): its amplitude response `A(w)` is small beyond the second
//! band (stop band) and `A(w)^2 + A(pi/M - w)^2 = 1` in the transition band (power complementarity), which cancels the overlap of neighbouring
//! bands. This module finds the `5 M` free coefficients of the prototype by a deterministic least-squares optimisation (Levenberg-Marquardt) of
//! exactly these two conditions, starting from a Kaiser-windowed sinc, and turns the result into the synthesis window `D` the codec uses
//! (`D = +-K h`, the sign flipping every `2 M` taps, `K` fixed so that analysis followed by synthesis has gain 1).
//!
//! No number in this file or in the starting point comes from any other codec's source, table or output.
// SPDX-License-Identifier: Apache-2.0

use std::f64::consts::PI;

/// Settings of the design, fixed: changing one changes the table, and a test pins the table against this procedure.
pub struct Design {
    /// Prototype taps (`10 M`).
    pub taps: usize,
    /// Stop band starts at `stop_edge * pi / M`.
    pub stop_edge: f64,
    /// Weight of the stop-band energy against the power-complementarity error.
    pub stop_weight: f64,
    pub grid: usize,
    pub iterations: usize,
}

impl Design {
    pub fn for_bands(m: usize) -> Design {
        // found by a sweep of 6 edges x 4 weights per band count (docs/development/SBC-TABLES.md): the same pair is best or near best for both
        let (stop_edge, stop_weight) = (1.1, 1.0);
        Design { taps: 10 * m, stop_edge, stop_weight, grid: 512, iterations: 300 }
    }
}

fn kaiser_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..60 {
        term *= (x / (2.0 * k as f64)).powi(2);
        sum += term;
    }
    sum
}

/// Amplitude response `A(w)` of the symmetric prototype from its free half `p[k] = h[5M - k]`.
pub fn amplitude(p: &[f64], w: f64) -> f64 {
    let mut a = p[0];
    for (k, pk) in p.iter().enumerate().skip(1) {
        a += 2.0 * pk * (k as f64 * w).cos();
    }
    a
}

/// Solves `m x = b` (Gaussian elimination, partial pivoting); `m` is row-major `n x n`.
fn solve(mut m: Vec<f64>, mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for c in 0..n {
        let piv = (c..n).max_by(|&a, &b2| m[a * n + c].abs().total_cmp(&m[b2 * n + c].abs()))?;
        if m[piv * n + c].abs() < 1e-300 {
            return None;
        }
        if piv != c {
            for k in 0..n {
                m.swap(c * n + k, piv * n + k);
            }
            b.swap(c, piv);
        }
        for r in c + 1..n {
            let f = m[r * n + c] / m[c * n + c];
            for k in c..n {
                m[r * n + k] -= f * m[c * n + k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for k in r + 1..n {
            s -= m[r * n + k] * x[k];
        }
        x[r] = s / m[r * n + r];
    }
    Some(x)
}

/// The residuals (and their Jacobian rows) of the two design conditions.
fn residuals(p: &[f64], m: usize, d: &Design) -> (Vec<f64>, Vec<Vec<f64>>) {
    let np = p.len();
    let (mut r, mut j) = (Vec::new(), Vec::new());
    let ws = d.stop_edge * PI / m as f64;
    let dw_stop = (PI - ws) / d.grid as f64;
    let basis = |w: f64| -> Vec<f64> { (0..np).map(|k| if k == 0 { 1.0 } else { 2.0 * (k as f64 * w).cos() }).collect() };
    for i in 0..d.grid {
        let w = ws + dw_stop * (i as f64 + 0.5);
        let f = (d.stop_weight * dw_stop).sqrt();
        r.push(f * amplitude(p, w));
        j.push(basis(w).into_iter().map(|v| v * f).collect());
    }
    let band = PI / m as f64;
    let dw = band / d.grid as f64;
    for i in 0..d.grid {
        let w = dw * (i as f64 + 0.5);
        let (a, b) = (amplitude(p, w), amplitude(p, band - w));
        let f = dw.sqrt();
        r.push(f * (a * a + b * b - 1.0));
        let (ba, bb) = (basis(w), basis(band - w));
        j.push((0..np).map(|k| f * (2.0 * a * ba[k] + 2.0 * b * bb[k])).collect());
    }
    (r, j)
}

/// The prototype lowpass filter of `m` bands, `10 m` taps (index 0 is 0), amplitude normalised to 1 in the pass band.
pub fn prototype(m: usize) -> Vec<f64> {
    prototype_with(m, &Design::for_bands(m))
}

pub fn prototype_with(m: usize, d: &Design) -> Vec<f64> {
    let n = d.taps;
    let np = n / 2;
    // start: Kaiser-windowed sinc, cut-off pi / (2 m)
    let wc = PI / (2.0 * m as f64);
    let beta = 8.0;
    let mut p: Vec<f64> = (0..np)
        .map(|k| {
            let t = k as f64;
            let sinc = if k == 0 { wc / PI } else { (wc * t).sin() / (PI * t) };
            let x = t / (n as f64 / 2.0);
            sinc * kaiser_i0(beta * (1.0 - x * x).max(0.0).sqrt()) / kaiser_i0(beta)
        })
        .collect();
    let cost = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
    let (mut r, mut j) = residuals(&p, m, d);
    let mut c = cost(&r);
    let mut mu = 1e-3;
    for _ in 0..d.iterations {
        let mut jtj = vec![0.0; np * np];
        let mut g = vec![0.0; np];
        for (row, rv) in j.iter().zip(&r) {
            for a in 0..np {
                g[a] -= row[a] * rv;
                for b in 0..np {
                    jtj[a * np + b] += row[a] * row[b];
                }
            }
        }
        let mut improved = false;
        for _ in 0..20 {
            let mut mm = jtj.clone();
            for a in 0..np {
                mm[a * np + a] += mu * jtj[a * np + a].max(1e-12);
            }
            if let Some(dx) = solve(mm, g.clone(), np) {
                let q: Vec<f64> = p.iter().zip(&dx).map(|(a, b)| a + b).collect();
                let (r2, j2) = residuals(&q, m, d);
                let c2 = cost(&r2);
                if c2 < c {
                    p = q;
                    r = r2;
                    j = j2;
                    let done = c - c2 < 1e-16 * c.max(1e-30);
                    c = c2;
                    mu = (mu / 3.0).max(1e-12);
                    improved = true;
                    if done {
                        return finish(&p, n);
                    }
                    break;
                }
            }
            mu *= 4.0;
        }
        if !improved {
            break;
        }
    }
    finish(&p, n)
}

fn finish(p: &[f64], n: usize) -> Vec<f64> {
    let mut h = vec![0.0; n];
    for (k, pk) in p.iter().enumerate() {
        if k == n / 2 {
            h[k] = *pk;
        } else if k > 0 {
            h[n / 2 - k] = *pk;
            h[n / 2 + k] = *pk;
        } else {
            h[n / 2] = *pk;
        }
    }
    h[0] = 0.0;
    h
}

/// The synthesis window from a prototype and a gain: `D[n] = -K h[n]` in blocks of `2 m` taps with even index, `+K h[n]` in the odd ones.
pub fn window_from(h: &[f64], m: usize, k: f64) -> Vec<f64> {
    h.iter().enumerate().map(|(n, v)| if (n / (2 * m)) % 2 == 0 { -k * v } else { k * v }).collect()
}

/// A deterministic test signal (a xorshift generator mapped to -1..1), identical on every machine.
pub fn noise(len: usize) -> Vec<f64> {
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        })
        .collect()
}

/// What the filter bank alone (analysis then synthesis, nothing quantised) does to a signal: the best delay, the gain at that delay and the
/// error against the delayed, gain-corrected input, in dB below the signal.
#[derive(Debug, Clone, Copy)]
pub struct LoopResult {
    pub delay: usize,
    pub gain: f64,
    pub error_db: f64,
}

pub fn measure_loop(m: usize, d: &[f64]) -> LoopResult {
    measure_pair(m, d, d)
}

/// As [`measure_loop`] with different windows in the analysis (an encoder) and the synthesis (a decoder).
pub fn measure_pair(m: usize, da: &[f64], ds: &[f64]) -> LoopResult {
    let len = 4096 * m;
    let x = noise(len);
    let y = crate::sbc::filter_bank_loop(m, da, ds, &x);
    let skip = 20 * m;
    let mut best = (0usize, 0.0f64);
    for lag in 0..12 * m {
        let c: f64 = (skip..len - lag).map(|i| x[i] * y[i + lag]).sum();
        if c.abs() > best.1.abs() {
            best = (lag, c);
        }
    }
    let lag = best.0;
    let xx: f64 = (skip..len - lag).map(|i| x[i] * x[i]).sum();
    let gain = best.1 / xx;
    let err: f64 = (skip..len - lag).map(|i| (y[i + lag] - gain * x[i]).powi(2)).sum();
    LoopResult { delay: lag, gain, error_db: 10.0 * (err / (gain * gain * xx)).log10() }
}

/// The designed synthesis window for `m` bands, gain set so that the loop has gain 1.
pub fn designed_window(m: usize) -> Vec<f64> {
    window_of(m, &prototype(m))
}

pub fn window_of(m: usize, h: &[f64]) -> Vec<f64> {
    let first = window_from(h, m, m as f64);
    let g = measure_loop(m, &first).gain;
    window_from(h, m, m as f64 / g.abs().sqrt())
}

#[cfg(test)]
#[path = "sbc_design_tests.rs"]
mod tests;
