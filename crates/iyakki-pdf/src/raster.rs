//! A small anti-aliased rasteriser: polygon coverage with 4 sub-scanlines and exact horizontal coverage, stroking by building
//! polygons (joins, caps, dashes), and clip masks. Everything is clamped to the canvas; point and edge counts are capped.

use crate::geom::flatten;
use crate::geom::Seg;

const SS: usize = 4;
const MAX_POINTS: usize = 3_000_000;

#[derive(Clone)]
pub struct Cov {
    pub x0: i32,
    pub y0: i32,
    pub w: i32,
    pub h: i32,
    pub data: Vec<u8>,
}

impl Cov {
    pub fn at(&self, x: i32, y: i32) -> u8 {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.w || y >= self.y0 + self.h {
            0
        } else {
            self.data[((y - self.y0) * self.w + (x - self.x0)) as usize]
        }
    }
}

/// The current clip: a box, and optionally a coverage mask inside it.
#[derive(Clone)]
pub struct Clip {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub mask: Option<std::rc::Rc<Cov>>,
}

impl Clip {
    pub fn full(w: u32, h: u32) -> Clip {
        Clip { x0: 0, y0: 0, x1: w as i32, y1: h as i32, mask: None }
    }
    pub fn empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
    pub fn alpha(&self, x: i32, y: i32) -> u8 {
        if x < self.x0 || y < self.y0 || x >= self.x1 || y >= self.y1 {
            return 0;
        }
        match &self.mask {
            Some(m) => m.at(x, y),
            None => 255,
        }
    }
}

pub fn polygons_of(path: &[Seg]) -> Vec<Vec<(f32, f32)>> {
    flatten(path).into_iter().map(|(p, _)| p).collect()
}

/// Coverage of the polygons (implicitly closed) under the fill rule, inside `bounds` (x0, y0, x1, y1).
pub fn coverage(polys: &[Vec<(f32, f32)>], even_odd: bool, bounds: (i32, i32, i32, i32)) -> Option<Cov> {
    let total: usize = polys.iter().map(|p| p.len()).sum();
    if total == 0 || total > MAX_POINTS {
        return None;
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in polys {
        for &(x, y) in p {
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
    }
    let bx0 = (minx.floor() as i64).max(bounds.0 as i64) as i32;
    let by0 = (miny.floor() as i64).max(bounds.1 as i64) as i32;
    let bx1 = (maxx.ceil() as i64).min(bounds.2 as i64) as i32;
    let by1 = (maxy.ceil() as i64).min(bounds.3 as i64) as i32;
    if bx0 >= bx1 || by0 >= by1 {
        return None;
    }
    let (w, h) = ((bx1 - bx0) as usize, (by1 - by0) as usize);
    let nsub = h * SS;
    let mut cross: Vec<Vec<(f32, i8)>> = vec![Vec::new(); nsub];
    for p in polys {
        if p.len() < 2 {
            continue;
        }
        for i in 0..p.len() {
            let (xa, ya) = p[i];
            let (xb, yb) = p[(i + 1) % p.len()];
            if ya == yb {
                continue;
            }
            let (dir, (x0, y0), (x1, y1)) = if ya < yb { (1i8, (xa, ya), (xb, yb)) } else { (-1i8, (xb, yb), (xa, ya)) };
            let rel0 = (y0 - by0 as f32) * SS as f32;
            let rel1 = (y1 - by0 as f32) * SS as f32;
            let s0 = ((rel0 - 0.5).ceil().max(0.0)) as usize;
            let s1 = (((rel1 - 0.5).ceil()).min(nsub as f32).max(0.0)) as usize;
            if s0 >= s1 {
                continue;
            }
            let slope = (x1 - x0) / (y1 - y0);
            for s in s0..s1 {
                let ys = by0 as f32 + (s as f32 + 0.5) / SS as f32;
                cross[s].push((x0 + (ys - y0) * slope, dir));
            }
        }
    }
    let mut data = vec![0u8; w * h];
    let mut acc = vec![0f32; w + 1];
    for row in 0..h {
        let mut any = false;
        for sub in 0..SS {
            let c = &mut cross[row * SS + sub];
            if c.is_empty() {
                continue;
            }
            c.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut wind = 0i32;
            for k in 0..c.len() - 1 {
                wind += c[k].1 as i32;
                let inside = if even_odd { (k + 1) % 2 == 1 } else { wind != 0 };
                if inside {
                    let xa = (c[k].0 - bx0 as f32).clamp(0.0, w as f32);
                    let xb = (c[k + 1].0 - bx0 as f32).clamp(0.0, w as f32);
                    if xb > xa {
                        any = true;
                        let (ia, ib) = (xa as usize, xb as usize);
                        if ia == ib {
                            acc[ia] += xb - xa;
                        } else {
                            acc[ia] += (ia + 1) as f32 - xa;
                            for v in acc.iter_mut().take(ib).skip(ia + 1) {
                                *v += 1.0;
                            }
                            acc[ib] += xb - ib as f32;
                        }
                    }
                }
            }
        }
        if any {
            for x in 0..w {
                data[row * w + x] = ((acc[x] / SS as f32).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                acc[x] = 0.0;
            }
            acc[w] = 0.0;
        }
    }
    Some(Cov { x0: bx0, y0: by0, w: w as i32, h: h as i32, data })
}

fn orient(mut p: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let mut a = 0f32;
    for i in 0..p.len() {
        let (x0, y0) = p[i];
        let (x1, y1) = p[(i + 1) % p.len()];
        a += x0 * y1 - x1 * y0;
    }
    if a < 0.0 {
        p.reverse();
    }
    p
}

fn circle(c: (f32, f32), r: f32) -> Vec<(f32, f32)> {
    let n = ((r * 3.0) as usize).clamp(8, 32);
    (0..n).map(|i| {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        (c.0 + r * a.cos(), c.1 + r * a.sin())
    }).collect()
}

pub struct StrokeStyle {
    pub width: f32,
    pub cap: i64,
    pub join: i64,
    pub miter: f32,
    pub dash: Vec<f32>,
    pub phase: f32,
}

fn dashed(poly: &[(f32, f32)], closed: bool, dash: &[f32], phase: f32) -> Vec<(Vec<(f32, f32)>, bool)> {
    let mut pts = poly.to_vec();
    if closed && pts.len() > 1 {
        pts.push(pts[0]);
    }
    let mut out = Vec::new();
    let mut idx = 0usize;
    let mut left = dash[0];
    let mut on = true;
    let mut ph = phase.max(0.0) % dash.iter().sum::<f32>().max(1e-3);
    while ph > 0.0 {
        if ph >= left {
            ph -= left;
            idx = (idx + 1) % dash.len();
            left = dash[idx];
            on = !on;
        } else {
            left -= ph;
            ph = 0.0;
        }
    }
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut count = 0usize;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if len <= 0.0 {
            continue;
        }
        let mut t = 0f32;
        if on && cur.is_empty() {
            cur.push(a);
        }
        while t < len {
            let step = (len - t).min(left);
            t += step;
            left -= step;
            let p = (a.0 + (b.0 - a.0) * t / len, a.1 + (b.1 - a.1) * t / len);
            if on {
                cur.push(p);
            }
            if left <= 1e-4 {
                if on && cur.len() >= 2 {
                    out.push((std::mem::take(&mut cur), false));
                }
                cur.clear();
                idx = (idx + 1) % dash.len();
                left = dash[idx].max(1e-3);
                on = !on;
                if on {
                    cur.push(p);
                }
                count += 1;
                if count > 200_000 {
                    return out;
                }
            }
        }
    }
    if on && cur.len() >= 2 {
        out.push((cur, false));
    }
    out
}

/// The polygons whose union (nonzero) is the stroke of `path`.
pub fn stroke_polys(path: &[Seg], st: &StrokeStyle) -> Vec<Vec<(f32, f32)>> {
    let hw = (st.width * 0.5).max(0.4);
    let mut out: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut lines = flatten(path);
    if !st.dash.is_empty() {
        let mut d = Vec::new();
        for (p, c) in &lines {
            d.extend(dashed(p, *c, &st.dash, st.phase));
        }
        lines = d;
    }
    for (pts, closed) in lines {
        // drop repeated points
        let mut p: Vec<(f32, f32)> = Vec::with_capacity(pts.len());
        for q in pts {
            if p.last().map(|l| (l.0 - q.0).abs() > 1e-4 || (l.1 - q.1).abs() > 1e-4).unwrap_or(true) {
                p.push(q);
            }
        }
        if closed && p.len() > 2 && p[0] == p[p.len() - 1] {
            p.pop();
        }
        if p.len() == 1 {
            // a dot: round and square caps show it
            match st.cap {
                1 => out.push(circle(p[0], hw)),
                2 => out.push(orient(vec![(p[0].0 - hw, p[0].1 - hw), (p[0].0 + hw, p[0].1 - hw), (p[0].0 + hw, p[0].1 + hw), (p[0].0 - hw, p[0].1 + hw)])),
                _ => {}
            }
            continue;
        }
        if p.len() < 2 {
            continue;
        }
        let n = p.len();
        let segs = if closed { n } else { n - 1 };
        let mut dirs: Vec<(f32, f32)> = Vec::with_capacity(segs);
        for i in 0..segs {
            let (a, b) = (p[i], p[(i + 1) % n]);
            let l = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt().max(1e-6);
            dirs.push(((b.0 - a.0) / l, (b.1 - a.1) / l));
        }
        for i in 0..segs {
            let (a, b) = (p[i], p[(i + 1) % n]);
            let (dx, dy) = dirs[i];
            let (nx, ny) = (-dy * hw, dx * hw);
            let (mut a2, mut b2) = (a, b);
            if !closed && st.cap == 2 {
                if i == 0 {
                    a2 = (a.0 - dx * hw, a.1 - dy * hw);
                }
                if i == segs - 1 {
                    b2 = (b.0 + dx * hw, b.1 + dy * hw);
                }
            }
            out.push(orient(vec![(a2.0 + nx, a2.1 + ny), (b2.0 + nx, b2.1 + ny), (b2.0 - nx, b2.1 - ny), (a2.0 - nx, a2.1 - ny)]));
        }
        if !closed && st.cap == 1 {
            out.push(circle(p[0], hw));
            out.push(circle(p[n - 1], hw));
        }
        // joins
        let joins: Vec<usize> = if closed { (0..n).collect() } else { (1..n - 1).collect() };
        for v in joins {
            let d0 = dirs[(v + segs - 1) % segs];
            let d1 = dirs[v % segs];
            let cross = d0.0 * d1.1 - d0.1 * d1.0;
            let dot = d0.0 * d1.0 + d0.1 * d1.1;
            if cross.abs() < 1e-4 && dot > 0.0 {
                continue;
            }
            let c = p[v];
            if st.join == 1 {
                out.push(circle(c, hw));
                continue;
            }
            let s = if cross > 0.0 { -1.0 } else { 1.0 };
            let (n0, n1) = ((-d0.1 * hw * s, d0.0 * hw * s), (-d1.1 * hw * s, d1.0 * hw * s));
            let a = (c.0 + n0.0, c.1 + n0.1);
            let b = (c.0 + n1.0, c.1 + n1.1);
            // miter length ratio = 1 / sin(theta/2), theta the angle between the segments
            let cos_t = -dot;
            let sin_half = ((1.0 - cos_t) / 2.0).max(0.0).sqrt();
            if st.join == 0 && sin_half > 1e-3 && 1.0 / sin_half <= st.miter {
                let k = 1.0 / (1.0 + dot).max(1e-3);
                let m = (c.0 + (n0.0 + n1.0) * k, c.1 + (n0.1 + n1.1) * k);
                out.push(orient(vec![c, a, m, b]));
            } else {
                out.push(orient(vec![c, a, b]));
            }
        }
    }
    out
}

/// A drawing surface: opaque RGBA, white to start.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas { w, h, rgba: vec![255; w as usize * h as usize * 4] }
    }

    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, rgb: [f32; 3], a: f32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || a <= 0.0 {
            return;
        }
        let i = (y as usize * self.w as usize + x as usize) * 4;
        for k in 0..3 {
            let d = self.rgba[i + k] as f32;
            self.rgba[i + k] = (rgb[k] * 255.0 * a + d * (1.0 - a) + 0.5).clamp(0.0, 255.0) as u8;
        }
    }

    pub fn fill_cov(&mut self, cov: &Cov, clip: &Clip, rgb: [f32; 3], alpha: f32) {
        for y in cov.y0.max(clip.y0)..(cov.y0 + cov.h).min(clip.y1) {
            for x in cov.x0.max(clip.x0)..(cov.x0 + cov.w).min(clip.x1) {
                let c = cov.data[((y - cov.y0) * cov.w + (x - cov.x0)) as usize];
                if c == 0 {
                    continue;
                }
                let m = match &clip.mask {
                    Some(m) => m.at(x, y),
                    None => 255,
                };
                if m == 0 {
                    continue;
                }
                self.blend(x, y, rgb, c as f32 / 255.0 * m as f32 / 255.0 * alpha);
            }
        }
    }
}

/// Intersect a clip with a path (device space).
pub fn clip_with(clip: &Clip, polys: &[Vec<(f32, f32)>], even_odd: bool) -> Clip {
    // an axis-aligned rectangle: just shrink the box
    if polys.len() == 1 && polys[0].len() == 4 {
        let p = &polys[0];
        let ax = (p[0].0 == p[1].0 && p[1].1 == p[2].1 && p[2].0 == p[3].0 && p[3].1 == p[0].1) || (p[0].1 == p[1].1 && p[1].0 == p[2].0 && p[2].1 == p[3].1 && p[3].0 == p[0].0);
        if ax {
            let xs = [p[0].0, p[1].0, p[2].0, p[3].0];
            let ys = [p[0].1, p[1].1, p[2].1, p[3].1];
            let fx = xs.iter().cloned().fold(f32::MAX, f32::min);
            let tx = xs.iter().cloned().fold(f32::MIN, f32::max);
            let fy = ys.iter().cloned().fold(f32::MAX, f32::min);
            let ty = ys.iter().cloned().fold(f32::MIN, f32::max);
            return Clip { x0: clip.x0.max(fx.round() as i32), y0: clip.y0.max(fy.round() as i32), x1: clip.x1.min(tx.round() as i32), y1: clip.y1.min(ty.round() as i32), mask: clip.mask.clone() };
        }
    }
    match coverage(polys, even_odd, (clip.x0, clip.y0, clip.x1, clip.y1)) {
        None => Clip { x0: 0, y0: 0, x1: 0, y1: 0, mask: None },
        Some(mut c) => {
            if let Some(old) = &clip.mask {
                for y in 0..c.h {
                    for x in 0..c.w {
                        let i = (y * c.w + x) as usize;
                        c.data[i] = ((c.data[i] as u32 * old.at(c.x0 + x, c.y0 + y) as u32) / 255) as u8;
                    }
                }
            }
            Clip { x0: c.x0, y0: c.y0, x1: c.x0 + c.w, y1: c.y0 + c.h, mask: Some(std::rc::Rc::new(c)) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(f32, f32)> {
        vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    }

    #[test]
    fn a_pixel_aligned_square_is_fully_covered_and_a_half_pixel_is_half() {
        let c = coverage(&[square(2.0, 2.0, 6.0, 6.0)], false, (0, 0, 10, 10)).unwrap();
        assert_eq!(c.at(3, 3), 255);
        assert_eq!(c.at(6, 3), 0);
        let h = coverage(&[square(2.5, 2.0, 4.0, 4.0)], false, (0, 0, 10, 10)).unwrap();
        let v = h.at(2, 2);
        assert!((120..=135).contains(&v), "{v}");
    }

    #[test]
    fn even_odd_makes_a_hole_and_nonzero_does_not() {
        let polys = [square(0.0, 0.0, 9.0, 9.0), square(3.0, 3.0, 6.0, 6.0)];
        let eo = coverage(&polys, true, (0, 0, 10, 10)).unwrap();
        assert_eq!(eo.at(4, 4), 0);
        assert_eq!(eo.at(1, 1), 255);
        let nz = coverage(&polys, false, (0, 0, 10, 10)).unwrap();
        assert_eq!(nz.at(4, 4), 255);
    }

    #[test]
    fn a_horizontal_line_stroke_has_its_width() {
        let st = StrokeStyle { width: 4.0, cap: 0, join: 0, miter: 10.0, dash: vec![], phase: 0.0 };
        let polys = stroke_polys(&[Seg::Move(2.0, 10.0), Seg::Line(18.0, 10.0)], &st);
        let c = coverage(&polys, false, (0, 0, 30, 30)).unwrap();
        assert_eq!(c.at(10, 9), 255);
        assert_eq!(c.at(10, 11), 255);
        assert_eq!(c.at(10, 7), 0);
        assert_eq!(c.at(1, 10), 0);
    }

    #[test]
    fn dashes_leave_gaps() {
        let st = StrokeStyle { width: 2.0, cap: 0, join: 0, miter: 10.0, dash: vec![4.0, 4.0], phase: 0.0 };
        let polys = stroke_polys(&[Seg::Move(0.0, 5.0), Seg::Line(32.0, 5.0)], &st);
        let c = coverage(&polys, false, (0, 0, 40, 10)).unwrap();
        assert_eq!(c.at(2, 5), 255);
        assert_eq!(c.at(6, 5), 0);
        assert_eq!(c.at(10, 5), 255);
    }

    #[test]
    fn a_rectangle_clip_shrinks_the_box_and_other_shapes_make_a_mask() {
        let full = Clip::full(20, 20);
        let r = clip_with(&full, &[square(5.0, 5.0, 10.0, 10.0)], false);
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (5, 5, 10, 10));
        assert!(r.mask.is_none());
        let tri = clip_with(&full, &[vec![(0.0, 0.0), (20.0, 0.0), (0.0, 20.0)]], false);
        assert!(tri.mask.is_some());
        assert_eq!(tri.alpha(2, 2), 255);
        assert_eq!(tri.alpha(18, 18), 0);
    }

    #[test]
    fn huge_and_nan_coordinates_do_not_panic() {
        let _ = coverage(&[vec![(f32::NAN, 0.0), (1e30, 1e30), (-1e30, 5.0)]], false, (0, 0, 10, 10));
        let _ = coverage(&[vec![(0.0, 0.0), (1e7, 0.0), (1e7, 1e7)]], true, (0, 0, 100, 100));
    }
}
