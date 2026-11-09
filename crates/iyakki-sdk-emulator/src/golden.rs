//! Golden pictures: compare a drawn picture with a saved PNG, with an optional tolerance, and report the hash.

use crate::png;
use crate::sha256::sha256_hex;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    /// Largest difference of one colour channel that still counts as the same pixel (0 = exact).
    pub channel: u8,
    /// Share of pixels, in percent of all pixels, allowed to differ by more than `channel`.
    pub percent: f64,
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance { channel: 0, percent: 0.0 }
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Byte for byte the same.
    Same,
    /// Different, but inside the tolerance: how many pixels differ and the largest channel difference.
    Within { pixels: usize, max: u8 },
    Different { pixels: usize, percent: f64, max: u8 },
    SizeMismatch { golden: (u32, u32), actual: (u32, u32) },
    Unreadable(String),
}

impl Outcome {
    pub fn passed(&self) -> bool {
        matches!(self, Outcome::Same | Outcome::Within { .. })
    }
}

/// SHA-256 of the RGBA bytes: the picture's identity in test output.
pub fn picture_hash(rgba: &[u8]) -> String {
    sha256_hex(rgba)
}

/// Compare `actual` (w x h RGBA) with the PNG bytes of a golden picture.
pub fn compare(w: u32, h: u32, actual: &[u8], golden_png: &[u8], tol: Tolerance) -> Outcome {
    let (gw, gh, g) = match png::decode(golden_png) {
        Ok(x) => x,
        Err(e) => return Outcome::Unreadable(e),
    };
    if (gw, gh) != (w, h) {
        return Outcome::SizeMismatch { golden: (gw, gh), actual: (w, h) };
    }
    if g == actual {
        return Outcome::Same;
    }
    let (mut pixels, mut max_all) = (0usize, 0u8);
    for (a, b) in actual.chunks(4).zip(g.chunks(4)) {
        let m = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        max_all = max_all.max(m);
        if m > tol.channel {
            pixels += 1;
        }
    }
    let total = (w as usize * h as usize).max(1);
    let percent = pixels as f64 * 100.0 / total as f64;
    if percent <= tol.percent && (pixels == 0 || tol.percent > 0.0) {
        Outcome::Within { pixels, max: max_all }
    } else {
        Outcome::Different { pixels, percent, max: max_all }
    }
}
