//! Paths and text clusters for the display list (version 3). Plain Rust with no pixel toolkit, so the client engine links it.
//!
//! A [`Seg`] list is a path: `M x:y`, `L x:y`, `Q cx:cy x:y`, `C c1x:c1y c2x:c2y x:y`, `Z`. The wire form is the commands joined with `;`
//! (`M0:0;L10:0;Q10:10:0:10;Z`). Every renderer flattens with [`flatten`] and fills with [`fill_mask`] or strokes with [`stroke_mask`], so
//! the pixel canvas, the engine and the tile adapter agree on the shape.

/// The most segments one path carries, and the most points a flattened path may have (a reader refuses more).
pub const MAX_SEGS: usize = 2048;
pub const MAX_POINTS: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

/// The wire text of a path (`Z` alone is a closed shape's end).
pub fn encode(segs: &[Seg]) -> String {
    let mut s = String::new();
    for (i, g) in segs.iter().enumerate() {
        if i > 0 {
            s.push(';');
        }
        match g {
            Seg::Move(x, y) => s += &format!("M{x}:{y}"),
            Seg::Line(x, y) => s += &format!("L{x}:{y}"),
            Seg::Quad(a, b, x, y) => s += &format!("Q{a}:{b}:{x}:{y}"),
            Seg::Cubic(a, b, c, d, x, y) => s += &format!("C{a}:{b}:{c}:{d}:{x}:{y}"),
            Seg::Close => s.push('Z'),
        }
    }
    s
}

/// A path from its wire text. A path starts with a move, has at most [`MAX_SEGS`] segments and only finite numbers.
pub fn decode(text: &str) -> Result<Vec<Seg>, String> {
    let mut out = Vec::new();
    for part in text.split(';') {
        if out.len() >= MAX_SEGS {
            return Err("a path has at most 2048 segments".into());
        }
        let mut ch = part.chars();
        let cmd = ch.next().ok_or("an empty path segment")?;
        let rest = ch.as_str();
        let nums = |n: usize| -> Result<Vec<f32>, String> {
            let v: Result<Vec<f32>, _> = rest.split(':').map(|t| t.parse::<f32>()).collect();
            let v = v.map_err(|_| format!("bad number in path segment '{part}'"))?;
            if v.len() != n || v.iter().any(|x| !x.is_finite()) {
                return Err(format!("path segment '{part}' needs {n} finite numbers"));
            }
            Ok(v)
        };
        let seg = match cmd {
            'M' => {
                let v = nums(2)?;
                Seg::Move(v[0], v[1])
            }
            'L' => {
                let v = nums(2)?;
                Seg::Line(v[0], v[1])
            }
            'Q' => {
                let v = nums(4)?;
                Seg::Quad(v[0], v[1], v[2], v[3])
            }
            'C' => {
                let v = nums(6)?;
                Seg::Cubic(v[0], v[1], v[2], v[3], v[4], v[5])
            }
            'Z' if rest.is_empty() => Seg::Close,
            _ => return Err(format!("unknown path segment '{part}'")),
        };
        if out.is_empty() && !matches!(seg, Seg::Move(..)) {
            return Err("a path starts with a move".into());
        }
        out.push(seg);
    }
    Ok(out)
}

/// One flattened subpath: its points and whether it was closed.
#[derive(Clone, Debug, PartialEq)]
pub struct Poly {
    pub pts: Vec<(f32, f32)>,
    pub closed: bool,
}

/// Curves cut into straight pieces no further than about a quarter of a unit from the curve (`tol` units).
pub fn flatten(segs: &[Seg], tol: f32) -> Vec<Poly> {
    let tol = tol.max(0.01);
    let mut polys: Vec<Poly> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut start = (0.0f32, 0.0f32);
    let mut pen = (0.0f32, 0.0f32);
    let total = |polys: &Vec<Poly>, cur: &Vec<(f32, f32)>| polys.iter().map(|p| p.pts.len()).sum::<usize>() + cur.len();
    let flush = |polys: &mut Vec<Poly>, cur: &mut Vec<(f32, f32)>, closed: bool| {
        if cur.len() > 1 || (closed && !cur.is_empty()) {
            polys.push(Poly { pts: std::mem::take(cur), closed });
        } else {
            cur.clear();
        }
    };
    for g in segs {
        if total(&polys, &cur) > MAX_POINTS {
            break;
        }
        match *g {
            Seg::Move(x, y) => {
                flush(&mut polys, &mut cur, false);
                start = (x, y);
                pen = start;
                cur.push(pen);
            }
            Seg::Line(x, y) => {
                cur.push((x, y));
                pen = (x, y);
            }
            Seg::Quad(cx, cy, x, y) => {
                let d = ((pen.0 - 2.0 * cx + x).hypot(pen.1 - 2.0 * cy + y)).max(0.0);
                let n = ((d / (4.0 * tol)).sqrt().ceil() as usize).clamp(1, 64);
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    cur.push((u * u * pen.0 + 2.0 * u * t * cx + t * t * x, u * u * pen.1 + 2.0 * u * t * cy + t * t * y));
                }
                pen = (x, y);
            }
            Seg::Cubic(ax, ay, bx, by, x, y) => {
                let d = ((pen.0 - 2.0 * ax + bx).hypot(pen.1 - 2.0 * ay + by)).max((ax - 2.0 * bx + x).hypot(ay - 2.0 * by + y));
                let n = ((3.0 * d / (4.0 * tol)).sqrt().ceil() as usize).clamp(1, 64);
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    cur.push((
                        u * u * u * pen.0 + 3.0 * u * u * t * ax + 3.0 * u * t * t * bx + t * t * t * x,
                        u * u * u * pen.1 + 3.0 * u * u * t * ay + 3.0 * u * t * t * by + t * t * t * y,
                    ));
                }
                pen = (x, y);
            }
            Seg::Close => {
                flush(&mut polys, &mut cur, true);
                pen = start;
                cur.push(pen);
            }
        }
    }
    flush(&mut polys, &mut cur, false);
    polys
}

/// The box a path covers, grown by `grow`: `(x0, y0, x1, y1)`.
pub fn bounds(polys: &[Poly], grow: f32) -> Option<(f32, f32, f32, f32)> {
    let mut it = polys.iter().flat_map(|p| p.pts.iter());
    let first = it.next()?;
    let mut b = (first.0, first.1, first.0, first.1);
    for p in it {
        b = (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1));
    }
    Some((b.0 - grow, b.1 - grow, b.2 + grow, b.3 + grow))
}

/// Coverage (0 to 1) of the filled shape for the pixels of a box: `(x0, y0)` is the box's top-left pixel, `w` by `h` pixels, row after
/// row. Non-zero winding, four sub-rows a pixel and exact horizontal coverage. Every subpath is closed for filling.
pub fn fill_mask(polys: &[Poly], x0: i32, y0: i32, w: usize, h: usize) -> Vec<f32> {
    const SUB: usize = 4;
    let mut mask = vec![0.0f32; w * h];
    if w == 0 || h == 0 {
        return mask;
    }
    for row in 0..h {
        for s in 0..SUB {
            let y = y0 as f32 + row as f32 + (s as f32 + 0.5) / SUB as f32;
            let mut cross: Vec<(f32, i32)> = Vec::new();
            for p in polys {
                let n = p.pts.len();
                for i in 0..n {
                    let (a, b) = (p.pts[i], p.pts[(i + 1) % n]);
                    if (a.1 <= y) != (b.1 <= y) {
                        let t = (y - a.1) / (b.1 - a.1);
                        cross.push((a.0 + t * (b.0 - a.0), if b.1 > a.1 { 1 } else { -1 }));
                    }
                }
            }
            cross.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            for k in 0..cross.len() {
                let before = wind;
                wind += cross[k].1;
                if before == 0 && wind != 0 {
                    // a span opens; it ends at the next crossing that brings the winding back to zero
                    let mut w2 = wind;
                    let mut j = k + 1;
                    while j < cross.len() {
                        w2 += cross[j].1;
                        if w2 == 0 {
                            break;
                        }
                        j += 1;
                    }
                    if j >= cross.len() {
                        break;
                    }
                    span(&mut mask, w, row, cross[k].0 - x0 as f32, cross[j].0 - x0 as f32, 1.0 / SUB as f32);
                }
            }
        }
    }
    for m in mask.iter_mut() {
        *m = m.min(1.0);
    }
    mask
}

fn span(mask: &mut [f32], w: usize, row: usize, a: f32, b: f32, weight: f32) {
    let (a, b) = (a.max(0.0), b.min(w as f32));
    if b <= a {
        return;
    }
    let (i0, i1) = (a.floor() as usize, (b.ceil() as usize).min(w));
    for i in i0..i1 {
        let ov = (b.min(i as f32 + 1.0) - a.max(i as f32)).max(0.0);
        mask[row * w + i] += ov * weight;
    }
}

/// Coverage of the outline: every straight piece is a line of `width` with round ends (the union, as the `line` operation draws one).
pub fn stroke_mask(polys: &[Poly], width: f32, x0: i32, y0: i32, w: usize, h: usize) -> Vec<f32> {
    let half = (width / 2.0).max(0.5);
    let mut mask = vec![0.0f32; w * h];
    for p in polys {
        let n = p.pts.len();
        let pieces = if p.closed { n } else { n.saturating_sub(1) };
        for i in 0..pieces.max(if n == 1 { 1 } else { 0 }) {
            let (a, b) = (p.pts[i], p.pts[(i + 1) % n]);
            let (bx0, by0) = ((a.0.min(b.0) - half - 1.0 - x0 as f32).floor().max(0.0) as usize, (a.1.min(b.1) - half - 1.0 - y0 as f32).floor().max(0.0) as usize);
            let (bx1, by1) = (((a.0.max(b.0) + half + 1.0 - x0 as f32).ceil().max(0.0) as usize).min(w), ((a.1.max(b.1) + half + 1.0 - y0 as f32).ceil().max(0.0) as usize).min(h));
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len2 = dx * dx + dy * dy;
            for yy in by0..by1 {
                for xx in bx0..bx1 {
                    let (qx, qy) = (xx as f32 + x0 as f32 + 0.5 - a.0, yy as f32 + y0 as f32 + 0.5 - a.1);
                    let t = if len2 > 0.0 { ((qx * dx + qy * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    let d = (qx - t * dx).hypot(qy - t * dy);
                    let c = (half + 0.5 - d).clamp(0.0, 1.0);
                    let m = &mut mask[yy * w + xx];
                    if c > *m {
                        *m = c;
                    }
                }
            }
        }
    }
    mask
}

/// The clusters of a run: a letter with the marks that belong to it (combining marks, variation selectors, a zero-width joiner and what it
/// joins, emoji skin tones, a pair of regional indicators, a Hangul syllable's jamo, a CR LF pair). Each entry is the byte range of one cluster.
pub fn clusters(s: &str) -> Vec<std::ops::Range<usize>> {
    fn extends(c: char) -> bool {
        matches!(c as u32,
            0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x200C | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F
            | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7 | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED
            | 0x0900..=0x0903 | 0x093A..=0x094F | 0x0951..=0x0957 | 0x0962..=0x0963 | 0x0B82 | 0x0BBE..=0x0BCD | 0x0BD7 | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E)
    }
    fn regional(c: char) -> bool {
        (0x1F1E6..=0x1F1FF).contains(&(c as u32))
    }
    fn jamo_l(c: char) -> bool {
        (0x1100..=0x115F).contains(&(c as u32))
    }
    fn jamo_vt(c: char) -> bool {
        (0x1160..=0x11FF).contains(&(c as u32))
    }
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    let mut prev: Option<char> = None;
    let mut regionals_in_cluster = 0usize;
    let mut join_next = false;
    for (i, c) in s.char_indices() {
        let end = i + c.len_utf8();
        let joins = match (prev, out.last()) {
            (Some(p), Some(_)) => {
                join_next
                    || extends(c)
                    || c == '\u{200D}'
                    || (p == '\r' && c == '\n')
                    || (regional(p) && regional(c) && regionals_in_cluster == 1)
                    || (jamo_l(p) && (jamo_l(c) || jamo_vt(c)))
                    || (jamo_vt(p) && jamo_vt(c))
            }
            _ => false,
        };
        if joins {
            let last = out.last_mut().expect("a cluster is open");
            last.end = end;
            if regional(c) {
                regionals_in_cluster += 1;
            }
        } else {
            out.push(i..end);
            regionals_in_cluster = usize::from(regional(c));
        }
        join_next = c == '\u{200D}';
        prev = Some(c);
    }
    out
}

/// The advance of each cluster of `s`, from a measure of whole strings (so kerning and shaping inside a cluster stay): the width of the
/// text up to the cluster's end minus the width up to its start.
pub fn cluster_advances(s: &str, mut measure: impl FnMut(&str) -> f32) -> Vec<f32> {
    let mut last = 0.0f32;
    clusters(s)
        .into_iter()
        .map(|r| {
            let w = measure(&s[..r.end]);
            let adv = (w - last).max(0.0);
            last = w;
            adv
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_round_trips_and_a_bad_one_is_refused_by_name() {
        let p = vec![Seg::Move(0.0, 0.0), Seg::Line(10.0, 0.5), Seg::Quad(10.0, 10.0, 0.0, 10.0), Seg::Cubic(1.0, 2.0, 3.0, 4.0, 5.0, 6.0), Seg::Close];
        assert_eq!(decode(&encode(&p)).unwrap(), p);
        assert!(decode("L1:1").unwrap_err().contains("starts with a move"));
        assert!(decode("M1:x").unwrap_err().contains("bad number"));
        assert!(decode("M1:2:3").unwrap_err().contains("needs 2"));
        assert!(decode("M0:0;Q1:2").unwrap_err().contains("needs 4"));
        assert!(decode("M0:0;X1:1").unwrap_err().contains("unknown"));
        assert!(decode("M0:inf").is_err());
        assert!(decode("M0:0;Z1").is_err());
        let long = format!("M0:0{}", ";L1:1".repeat(MAX_SEGS));
        assert!(decode(&long).unwrap_err().contains("2048"));
    }

    #[test]
    fn a_square_fills_exactly_and_a_triangle_covers_half_its_box() {
        let sq = flatten(&[Seg::Move(1.0, 1.0), Seg::Line(5.0, 1.0), Seg::Line(5.0, 5.0), Seg::Line(1.0, 5.0), Seg::Close], 0.25);
        let m = fill_mask(&sq, 0, 0, 6, 6);
        assert_eq!(m[2 * 6 + 2], 1.0);
        assert_eq!(m[0], 0.0);
        assert_eq!(m[5 * 6 + 5], 0.0);
        let tri = flatten(&[Seg::Move(0.0, 0.0), Seg::Line(8.0, 0.0), Seg::Line(0.0, 8.0), Seg::Close], 0.25);
        let area: f32 = fill_mask(&tri, 0, 0, 8, 8).iter().sum();
        assert!((area - 32.0).abs() < 1.0, "{area}");
    }

    #[test]
    fn a_hole_wound_the_other_way_stays_empty_and_curves_are_cut_in_pieces() {
        let ring = flatten(
            &[Seg::Move(0.0, 0.0), Seg::Line(9.0, 0.0), Seg::Line(9.0, 9.0), Seg::Line(0.0, 9.0), Seg::Close, Seg::Move(3.0, 3.0), Seg::Line(3.0, 6.0), Seg::Line(6.0, 6.0), Seg::Line(6.0, 3.0), Seg::Close],
            0.25,
        );
        let m = fill_mask(&ring, 0, 0, 9, 9);
        assert_eq!(m[4 * 9 + 4], 0.0);
        assert_eq!(m[9], 1.0);
        let arc = flatten(&[Seg::Move(0.0, 0.0), Seg::Quad(10.0, 0.0, 10.0, 10.0)], 0.25);
        assert!(arc[0].pts.len() > 4 && !arc[0].closed);
        let c = flatten(&[Seg::Move(0.0, 0.0), Seg::Cubic(0.0, 10.0, 10.0, 10.0, 10.0, 0.0)], 0.25);
        assert!(c[0].pts.len() > 4);
    }

    #[test]
    fn a_stroke_is_a_line_of_its_width_and_a_huge_path_is_cut() {
        let line = flatten(&[Seg::Move(0.0, 5.0), Seg::Line(10.0, 5.0)], 0.25);
        let m = stroke_mask(&line, 2.0, 0, 0, 10, 10);
        assert_eq!(m[5 * 10 + 5], 1.0);
        assert_eq!(m[5], 0.0);
        let many: Vec<Seg> = std::iter::once(Seg::Move(0.0, 0.0)).chain((0..MAX_SEGS).map(|i| Seg::Cubic(i as f32, 0.0, 0.0, i as f32, 1.0, 1.0))).collect();
        let total: usize = flatten(&many, 0.01).iter().map(|p| p.pts.len()).sum();
        assert!(total <= MAX_POINTS + 70);
    }

    #[test]
    fn clusters_keep_marks_joiners_flags_and_jamo_with_their_letter() {
        let n = |s: &str| clusters(s).len();
        assert_eq!(n("abc"), 3);
        assert_eq!(n("e\u{0301}a"), 2);
        assert_eq!(n("\u{1F1E9}\u{1F1EA}\u{1F1EB}\u{1F1F7}"), 2);
        assert_eq!(n("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"), 1);
        assert_eq!(n("\u{1F44D}\u{1F3FD}"), 1);
        assert_eq!(n("\u{1112}\u{1161}\u{11AB}"), 1);
        assert_eq!(n("a\r\nb"), 3);
        assert_eq!(n("\u{0915}\u{094D}"), 1);
        assert_eq!(n(""), 0);
        let r = clusters("e\u{0301}a");
        assert_eq!(&"e\u{0301}a"[r[0].clone()], "e\u{0301}");
    }

    #[test]
    fn the_advance_of_a_cluster_is_the_difference_of_the_measure_so_kerning_stays() {
        let adv = cluster_advances("ab", |s| if s == "ab" { 9.0 } else { 5.0 * s.chars().count() as f32 });
        assert_eq!(adv, vec![5.0, 4.0]);
    }
}
