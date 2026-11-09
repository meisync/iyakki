//! Zones and privacy masks: polygons in picture-relative coordinates (0.0 to 1.0 from the top left), so they do not depend on the
//! size of the picture a camera happens to send. Pure code (no files, no network, no serde) so it can be tested and measured alone.
//!
//! * A polygon is checked before it is kept: 3 to [`MAX_POINTS`] corners, every corner inside the picture, no repeated corner, an
//!   area of at least [`MIN_AREA`] of the picture, and no crossing edges (a bow tie has no clear inside). Anything else is refused
//!   in words.
//! * "Inside" is the even-odd rule at the middle of a pixel or cell. A point exactly on an edge may count either way.
//! * [`blacken`] paints the inside of masks black in an RGBA picture. It is the one function that makes a privacy mask real: the
//!   detector, the thumbnails, and (when the person asked) recordings and snapshots only ever see pictures that passed through it.

pub const MAX_POINTS: usize = 32;
/// Zones and masks per camera (each kind).
pub const MAX_SHAPES: usize = 12;
/// Smallest polygon area, as a fraction of the picture.
pub const MIN_AREA: f32 = 0.0005;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub fn new(x: f32, y: f32) -> Pt {
        Pt { x, y }
    }
}

/// A named zone. With at least one `include` zone, motion counts only inside the include zones; `exclude` zones never count.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub name: String,
    pub include: bool,
    /// 1 (least sensitive) to 10 (most); 5 is the camera's own setting.
    pub sensitivity: u8,
    pub points: Vec<Pt>,
}

/// A privacy mask: blacked out before anything looks at the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub name: String,
    pub points: Vec<Pt>,
}

/// Area of a polygon (shoelace), as a fraction of the picture.
pub fn area(p: &[Pt]) -> f32 {
    let n = p.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0f64;
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        s += a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64;
    }
    (s.abs() / 2.0) as f32
}

fn cross(o: Pt, a: Pt, b: Pt) -> f64 {
    (a.x as f64 - o.x as f64) * (b.y as f64 - o.y as f64) - (a.y as f64 - o.y as f64) * (b.x as f64 - o.x as f64)
}

fn on_segment(a: Pt, b: Pt, p: Pt) -> bool {
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// Whether the closed segments a-b and c-d share a point (touching counts).
fn segments_meet(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return true;
    }
    (d1 == 0.0 && on_segment(c, d, a)) || (d2 == 0.0 && on_segment(c, d, b)) || (d3 == 0.0 && on_segment(a, b, c)) || (d4 == 0.0 && on_segment(a, b, d))
}

/// Check a polygon; the error says what is wrong in words a person can act on.
pub fn validate(points: &[Pt]) -> Result<(), String> {
    let n = points.len();
    if n < 3 {
        return Err("a shape needs at least 3 corners".into());
    }
    if n > MAX_POINTS {
        return Err(format!("a shape has at most {MAX_POINTS} corners"));
    }
    for p in points {
        if !p.x.is_finite() || !p.y.is_finite() || p.x < 0.0 || p.x > 1.0 || p.y < 0.0 || p.y > 1.0 {
            return Err("every corner must lie inside the picture".into());
        }
    }
    for i in 0..n {
        if points[i] == points[(i + 1) % n] {
            return Err("two corners in a row are the same point".into());
        }
    }
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        for j in (i + 2)..n {
            if i == 0 && j == n - 1 {
                continue; // these two edges are neighbours
            }
            if segments_meet(a, b, points[j], points[(j + 1) % n]) {
                return Err("the shape crosses itself; draw the outline without crossing lines".into());
            }
        }
    }
    if area(points) < MIN_AREA {
        return Err("the shape is too small or flat (all corners on one line)".into());
    }
    for i in 0..n {
        // two neighbouring edges must not fold back over each other
        let (a, b, c) = (points[i], points[(i + 1) % n], points[(i + 2) % n]);
        if cross(a, b, c) == 0.0 && (b.x as f64 - a.x as f64) * (c.x as f64 - b.x as f64) + (b.y as f64 - a.y as f64) * (c.y as f64 - b.y as f64) < 0.0 {
            return Err("the shape folds back on itself".into());
        }
    }
    Ok(())
}

/// Even-odd point in polygon.
pub fn contains(poly: &[Pt], x: f32, y: f32) -> bool {
    let n = poly.len();
    let mut inside = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > y) != (b.y > y) {
            let xs = (b.x - a.x) * (y - a.y) / (b.y - a.y) + a.x;
            if x < xs {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// For each pixel row, the pixel ranges `[from, to)` whose centre lies inside the polygon.
pub fn spans(poly: &[Pt], w: u32, h: u32) -> Vec<Vec<(u32, u32)>> {
    let n = poly.len();
    let mut rows = vec![Vec::new(); h as usize];
    if n < 3 || w == 0 || h == 0 {
        return rows;
    }
    let (miny, maxy) = poly.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.y), hi.max(p.y)));
    let y0 = ((miny * h as f32 - 0.5).floor().max(0.0)) as u32;
    let y1 = (((maxy * h as f32 + 0.5).ceil()) as u32).min(h);
    let mut xs: Vec<f32> = Vec::with_capacity(n);
    for y in y0..y1 {
        let sy = (y as f32 + 0.5) / h as f32;
        xs.clear();
        let mut j = n - 1;
        for i in 0..n {
            let (a, b) = (poly[i], poly[j]);
            if (a.y > sy) != (b.y > sy) {
                xs.push((b.x - a.x) * (sy - a.y) / (b.y - a.y) + a.x);
            }
            j = i;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks_exact(2) {
            // pixel centre (px + 0.5) / w in [pair[0], pair[1])
            let from = (pair[0] * w as f32 - 0.5).ceil().max(0.0) as u32;
            let to = ((pair[1] * w as f32 - 0.5).ceil().max(0.0) as u32).min(w);
            if from < to {
                rows[y as usize].push((from, to));
            }
        }
    }
    rows
}

/// Paint the inside of every mask black (red, green and blue 0; alpha untouched). `rgba` is `w * h * 4` bytes; anything shorter is
/// left alone and `false` is returned (the caller must then not use the picture).
pub fn blacken(w: u32, h: u32, rgba: &mut [u8], masks: &[&[Pt]]) -> bool {
    if rgba.len() < w as usize * h as usize * 4 {
        return false;
    }
    for m in masks {
        for (y, row) in spans(m, w, h).iter().enumerate() {
            for &(a, b) in row {
                let base = y * w as usize * 4;
                for x in a as usize..b as usize {
                    let o = base + x * 4;
                    rgba[o] = 0;
                    rgba[o + 1] = 0;
                    rgba[o + 2] = 0;
                }
            }
        }
    }
    true
}

/// A name for a zone or mask: 1 to 40 printable characters.
pub fn clean_name(s: &str) -> Result<String, String> {
    let t: String = s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string();
    if t.is_empty() {
        return Err("give the shape a name".into());
    }
    if t.chars().count() > 40 {
        return Err("a name has at most 40 characters".into());
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Pt> {
        vec![Pt::new(x0, y0), Pt::new(x1, y0), Pt::new(x1, y1), Pt::new(x0, y1)]
    }

    #[test]
    fn points_are_inside_or_outside_a_square_and_a_concave_shape() {
        let sq = rect(0.2, 0.2, 0.6, 0.6);
        assert!(contains(&sq, 0.4, 0.4));
        assert!(!contains(&sq, 0.1, 0.4) && !contains(&sq, 0.4, 0.7) && !contains(&sq, 0.7, 0.4));
        // an L shape: the notch is outside
        let l = vec![Pt::new(0.0, 0.0), Pt::new(0.6, 0.0), Pt::new(0.6, 0.3), Pt::new(0.3, 0.3), Pt::new(0.3, 0.9), Pt::new(0.0, 0.9)];
        assert!(validate(&l).is_ok());
        assert!(contains(&l, 0.1, 0.7) && contains(&l, 0.5, 0.1));
        assert!(!contains(&l, 0.5, 0.6), "the notch of the L is outside");
        // a triangle and the area of shapes
        let tri = vec![Pt::new(0.0, 0.0), Pt::new(1.0, 0.0), Pt::new(0.0, 1.0)];
        assert!(contains(&tri, 0.2, 0.2) && !contains(&tri, 0.8, 0.8));
        assert!((area(&tri) - 0.5).abs() < 1e-6 && (area(&sq) - 0.16).abs() < 1e-6);
    }

    #[test]
    fn bad_polygons_are_refused_in_words() {
        assert!(validate(&[Pt::new(0.1, 0.1), Pt::new(0.5, 0.5)]).unwrap_err().contains("at least 3"));
        // a bow tie crosses itself
        let bow = vec![Pt::new(0.1, 0.1), Pt::new(0.9, 0.9), Pt::new(0.9, 0.1), Pt::new(0.1, 0.9)];
        assert!(validate(&bow).unwrap_err().contains("crosses itself"));
        // outside the picture, not a number
        assert!(validate(&rect(0.1, 0.1, 1.2, 0.5)).unwrap_err().contains("inside the picture"));
        assert!(validate(&[Pt::new(f32::NAN, 0.1), Pt::new(0.5, 0.1), Pt::new(0.5, 0.5)]).is_err());
        // all on one line, or a sliver
        assert!(validate(&[Pt::new(0.1, 0.1), Pt::new(0.5, 0.5), Pt::new(0.9, 0.9)]).unwrap_err().contains("too small or flat"));
        assert!(validate(&rect(0.1, 0.1, 0.11, 0.11)).unwrap_err().contains("too small"));
        // the same corner twice in a row
        assert!(validate(&[Pt::new(0.1, 0.1), Pt::new(0.1, 0.1), Pt::new(0.5, 0.1), Pt::new(0.5, 0.5)]).unwrap_err().contains("same point"));
        // too many corners
        let many: Vec<Pt> = (0..40).map(|i| { let a = i as f32 / 40.0 * std::f32::consts::TAU; Pt::new(0.5 + 0.4 * a.cos(), 0.5 + 0.4 * a.sin()) }).collect();
        assert!(validate(&many).unwrap_err().contains("at most"));
        // an edge that folds back over its neighbour
        let fold = vec![Pt::new(0.1, 0.1), Pt::new(0.8, 0.1), Pt::new(0.4, 0.1), Pt::new(0.4, 0.8)];
        assert!(validate(&fold).is_err());
        // good ones pass
        assert!(validate(&rect(0.1, 0.1, 0.9, 0.9)).is_ok());
        // names
        assert!(clean_name("  ").is_err() && clean_name(&"x".repeat(41)).is_err());
        assert_eq!(clean_name(" Drive\u{7}way ").unwrap(), "Driveway");
    }

    #[test]
    fn spans_agree_with_point_in_polygon_at_pixel_centres() {
        let l = vec![Pt::new(0.05, 0.1), Pt::new(0.7, 0.1), Pt::new(0.7, 0.4), Pt::new(0.35, 0.4), Pt::new(0.35, 0.95), Pt::new(0.05, 0.95)];
        let (w, h) = (97u32, 61u32);
        let rows = spans(&l, w, h);
        let mut mismatches = 0;
        for y in 0..h {
            for x in 0..w {
                let inside = rows[y as usize].iter().any(|&(a, b)| x >= a && x < b);
                if inside != contains(&l, (x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32) {
                    mismatches += 1;
                }
            }
        }
        assert!(mismatches <= 2, "{mismatches} pixels differ on an edge");
    }

    #[test]
    fn blackening_paints_only_inside_the_mask_and_keeps_alpha() {
        let (w, h) = (40u32, 30u32);
        let mut img = vec![200u8; (w * h * 4) as usize];
        let m = rect(0.25, 0.25, 0.75, 0.75);
        assert!(blacken(w, h, &mut img, &[&m]));
        let px = |x: u32, y: u32| &img[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize];
        assert_eq!(px(20, 15), [0, 0, 0, 200], "inside is black, alpha stays");
        assert_eq!(px(2, 2), [200; 4]);
        assert_eq!(px(38, 28), [200; 4]);
        let black = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| px(x, y)[0] == 0).count();
        assert_eq!(black, 20 * 15, "half the width and height");
        // a buffer that is too short is refused, not half painted
        let mut short = vec![9u8; 10];
        assert!(!blacken(w, h, &mut short, &[&m]) && short.iter().all(|b| *b == 9));
    }
}
