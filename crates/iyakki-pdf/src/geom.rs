//! Matrices and paths.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix(pub [f64; 6]);

impl Matrix {
    pub const ID: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    /// `self` applied first, then `o` (PDF row-vector convention: `self x o`).
    pub fn then(&self, o: &Matrix) -> Matrix {
        let (a, b) = (&self.0, &o.0);
        Matrix([
            a[0] * b[0] + a[1] * b[2],
            a[0] * b[1] + a[1] * b[3],
            a[2] * b[0] + a[3] * b[2],
            a[2] * b[1] + a[3] * b[3],
            a[4] * b[0] + a[5] * b[2] + b[4],
            a[4] * b[1] + a[5] * b[3] + b[5],
        ])
    }
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.0;
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }
    pub fn det(&self) -> f64 {
        self.0[0] * self.0[3] - self.0[1] * self.0[2]
    }
    pub fn invert(&self) -> Option<Matrix> {
        let d = self.det();
        if !d.is_finite() || d.abs() < 1e-12 {
            return None;
        }
        let m = &self.0;
        Some(Matrix([m[3] / d, -m[1] / d, -m[2] / d, m[0] / d, (m[2] * m[5] - m[3] * m[4]) / d, (m[1] * m[4] - m[0] * m[5]) / d]))
    }
    /// How much a length grows on average.
    pub fn scale(&self) -> f64 {
        self.det().abs().sqrt()
    }
    pub fn from_arr(a: &[f64]) -> Option<Matrix> {
        if a.len() >= 6 && a[..6].iter().all(|v| v.is_finite()) {
            Some(Matrix([a[0], a[1], a[2], a[3], a[4], a[5]]))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Curve(f64, f64, f64, f64, f64, f64),
    Close,
}

pub type Path = Vec<Seg>;

pub const MAX_PATH_SEGS: usize = 400_000;

/// Flatten into polylines (device space). Each is (points, closed).
pub fn flatten(path: &[Seg]) -> Vec<(Vec<(f32, f32)>, bool)> {
    let mut out: Vec<(Vec<(f32, f32)>, bool)> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut start = (0f64, 0f64);
    let mut last = (0f64, 0f64);
    let clampv = |v: f64| v.clamp(-1e7, 1e7) as f32;
    for s in path {
        match *s {
            Seg::Move(x, y) => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), false));
                }
                start = (x, y);
                last = (x, y);
                cur.push((clampv(x), clampv(y)));
            }
            Seg::Line(x, y) => {
                if cur.is_empty() {
                    cur.push((clampv(last.0), clampv(last.1)));
                }
                cur.push((clampv(x), clampv(y)));
                last = (x, y);
            }
            Seg::Curve(x1, y1, x2, y2, x3, y3) => {
                if cur.is_empty() {
                    cur.push((clampv(last.0), clampv(last.1)));
                }
                let len = dist(last, (x1, y1)) + dist((x1, y1), (x2, y2)) + dist((x2, y2), (x3, y3));
                let n = ((len / 3.0).sqrt().ceil() as usize).clamp(2, 48);
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let u = 1.0 - t;
                    let x = u * u * u * last.0 + 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t * x3;
                    let y = u * u * u * last.1 + 3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t * y3;
                    cur.push((clampv(x), clampv(y)));
                }
                last = (x3, y3);
            }
            Seg::Close => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), true));
                }
                last = start;
            }
        }
    }
    if !cur.is_empty() {
        out.push((cur, false));
    }
    out
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    if d.is_finite() {
        d.min(1e7)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrices_compose_and_invert() {
        let m = Matrix([2.0, 0.0, 0.0, 3.0, 10.0, 20.0]);
        assert_eq!(m.apply(1.0, 1.0), (12.0, 23.0));
        let i = m.invert().unwrap();
        let (x, y) = i.apply(12.0, 23.0);
        assert!((x - 1.0).abs() < 1e-9 && (y - 1.0).abs() < 1e-9);
        assert!(Matrix([0.0; 6]).invert().is_none());
        assert_eq!(Matrix::ID.then(&m), m);
    }

    #[test]
    fn a_curve_is_flattened_to_several_points() {
        let p = vec![Seg::Move(0.0, 0.0), Seg::Curve(0.0, 50.0, 100.0, 50.0, 100.0, 0.0)];
        let f = flatten(&p);
        assert_eq!(f.len(), 1);
        assert!(f[0].0.len() > 4);
    }
}
