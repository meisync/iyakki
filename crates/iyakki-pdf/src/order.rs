//! Reading order for a page's text lines (F-PDF-08): a page set in columns is read column by column, a heading or caption that
//! runs across the columns keeps its place between them, and a page that is one column (or a table with few rows) is left as it came.
//!
//! The method is plain geometry on the lines the interpreter produced. A line is cut into segments where it has a very wide gap;
//! the narrow segments are projected on the x axis and the strips nobody covers are gutters; a gutter only counts when at least
//! `MIN_PER_COLUMN` segments stand on each side of it (so a table of three rows is not read down its columns).

use crate::text::{Cell, TextLine};

/// A gap wider than this many font sizes ends a segment.
const GAP: f64 = 2.5;
/// A gutter is at least this many font sizes wide.
const GUTTER: f64 = 1.5;
/// Fewest narrow segments a column must hold, else its gutter is ignored.
const MIN_PER_COLUMN: usize = 4;
/// A segment wider than this part of the text block is "across" (a heading, a caption), not in a column.
const NARROW: f64 = 0.7;
/// Pages with more lines than this are returned as they are.
const MAX_LINES: usize = 20_000;

struct Seg {
    cells: Vec<Cell>,
    x0: f64,
    x1: f64,
    y: f64,
    size: f64,
    src: usize,
}

fn finite(l: &TextLine) -> bool {
    l.y.is_finite() && l.size.is_finite() && l.cells.iter().all(|c| c.x.is_finite() && c.w.is_finite())
}

fn segments(lines: &[TextLine]) -> Vec<Seg> {
    let mut out = Vec::new();
    for (src, l) in lines.iter().enumerate() {
        let mut cur: Vec<Cell> = Vec::new();
        let flush = |cur: &mut Vec<Cell>, out: &mut Vec<Seg>| {
            while cur.last().is_some_and(|c| c.text.trim().is_empty()) {
                cur.pop();
            }
            if cur.is_empty() {
                return;
            }
            let x0 = cur.iter().map(|c| c.x).fold(f64::MAX, f64::min);
            let x1 = cur.iter().map(|c| c.x + c.w.max(0.0)).fold(f64::MIN, f64::max);
            out.push(Seg { cells: std::mem::take(cur), x0, x1, y: l.y, size: l.size, src });
        };
        for c in &l.cells {
            if c.text.trim().is_empty() && c.w > GAP * l.size {
                flush(&mut cur, &mut out);
            } else if !(cur.is_empty() && c.text.trim().is_empty()) {
                cur.push(c.clone());
            }
        }
        flush(&mut cur, &mut out);
    }
    out
}

/// The uncovered strips (not touching the ends) of the covered intervals, at least `min_w` wide.
fn gaps(mut cover: Vec<(f64, f64)>, lo: f64, min_w: f64) -> Vec<(f64, f64)> {
    cover.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    let mut at = lo;
    for (a, b) in cover {
        if a - at >= min_w && at > lo {
            out.push((at, a));
        }
        at = at.max(b);
    }
    out
}

/// The lines of a page in reading order. Unchanged when the page has no column gutter.
pub fn reading_order(lines: Vec<TextLine>) -> Vec<TextLine> {
    if lines.len() < MIN_PER_COLUMN || lines.len() > MAX_LINES || !lines.iter().all(finite) {
        return lines;
    }
    let segs = segments(&lines);
    let (lo, hi) = segs.iter().fold((f64::MAX, f64::MIN), |(l, h), s| (l.min(s.x0), h.max(s.x1)));
    if hi <= lo {
        return lines;
    }
    let block = hi - lo;
    let mut sizes: Vec<f64> = segs.iter().map(|s| s.size).collect();
    sizes.sort_by(|a, b| a.total_cmp(b));
    let med = sizes[sizes.len() / 2].max(1.0);
    let narrow: Vec<&Seg> = segs.iter().filter(|s| s.x1 - s.x0 < NARROW * block).collect();
    let mut gutters = gaps(narrow.iter().map(|s| (s.x0, s.x1)).collect(), lo, GUTTER * med);
    gutters.retain(|g| g.1 < hi);
    // drop gutters that do not have enough text on both sides
    loop {
        let bounds = |i: usize| (if i == 0 { lo } else { gutters[i - 1].1 }, gutters.get(i).map_or(hi, |g| g.0));
        let bad = (0..=gutters.len()).find(|&i| {
            let (a, b) = bounds(i);
            narrow.iter().filter(|s| s.x0 >= a - 0.01 && s.x1 <= b + 0.01).count() < MIN_PER_COLUMN
        });
        match bad {
            None => break,
            Some(_) if gutters.is_empty() => return lines,
            Some(i) => {
                gutters.remove(i.saturating_sub(1).min(gutters.len() - 1));
            }
        }
    }
    if gutters.is_empty() {
        return lines;
    }
    let column = |s: &Seg| -> Option<usize> {
        if gutters.iter().any(|g| s.x0 < g.1 && s.x1 > g.0) {
            return None; // reaches into a gutter: across
        }
        Some(gutters.iter().filter(|g| g.1 <= s.x0 + 0.01).count())
    };
    let mut order: Vec<usize> = (0..segs.len()).collect();
    order.sort_by(|&a, &b| segs[b].y.total_cmp(&segs[a].y).then(segs[a].src.cmp(&segs[b].src)).then(segs[a].x0.total_cmp(&segs[b].x0)));
    let mut out: Vec<TextLine> = Vec::new();
    let mut band: Vec<Vec<usize>> = vec![Vec::new(); gutters.len() + 1];
    let emit_band = |band: &mut Vec<Vec<usize>>, out: &mut Vec<TextLine>| {
        for col in band.iter_mut() {
            let mut last_src = usize::MAX;
            let mut started = false;
            for &i in col.iter() {
                let s = &segs[i];
                if started && s.src == last_src {
                    // the same printed line, cut by a wide gap inside the column: joined again
                    let l = out.last_mut().unwrap();
                    l.cells.push(Cell { text: " ".into(), x: l.cells.last().map_or(s.x0, |c| c.x + c.w), w: 0.0 });
                    l.cells.extend(s.cells.iter().cloned());
                } else {
                    out.push(TextLine { cells: s.cells.clone(), y: s.y, size: s.size });
                }
                started = true;
                last_src = s.src;
            }
            col.clear();
        }
    };
    for i in order {
        match column(&segs[i]) {
            Some(c) => band[c].push(i),
            None => {
                emit_band(&mut band, &mut out);
                let s = &segs[i];
                out.push(TextLine { cells: s.cells.clone(), y: s.y, size: s.size });
            }
        }
    }
    emit_band(&mut band, &mut out);
    out
}
