//! Text extraction and search: glyphs from the interpreter are grouped into lines (new line when the baseline moves, a space where
//! the gap is wide) and lines into a page's text. Positions are kept so that search hits can be highlighted.

use crate::interp::{GState, Glyph, Sink};

#[derive(Clone, Debug)]
pub struct Cell {
    pub text: String,
    pub x: f64,
    pub w: f64,
}

#[derive(Clone, Debug)]
pub struct TextLine {
    pub cells: Vec<Cell>,
    pub y: f64,
    pub size: f64,
}

impl TextLine {
    pub fn text(&self) -> String {
        self.cells.iter().map(|c| c.text.as_str()).collect()
    }
}

#[derive(Default)]
pub struct TextSink {
    pub lines: Vec<TextLine>,
    last_end: f64,
    pub glyph_count: usize,
}

pub const MAX_GLYPHS: usize = 3_000_000;

impl Sink for TextSink {
    fn glyph(&mut self, g: &Glyph, _gs: &GState) {
        if g.text.is_empty() || self.glyph_count >= MAX_GLYPHS {
            return;
        }
        self.glyph_count += 1;
        let (x0, y0) = g.trm.apply(0.0, 0.0);
        let (x1, y1) = g.trm.apply(g.advance, 0.0);
        let (ux, uy) = g.trm.apply(0.0, 1.0);
        let size = ((ux - x0).powi(2) + (uy - y0).powi(2)).sqrt().max(0.01);
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
            return;
        }
        let w = (x1 - x0).abs();
        let vertical = (y1 - y0).abs() > (x1 - x0).abs();
        let x = x0.min(x1);
        let mut new_line = true;
        if let Some(cur) = self.lines.last_mut() {
            let tol = 0.45 * size.max(cur.size);
            if !vertical && (y0 - cur.y).abs() <= tol && x >= self.last_end - 2.5 * size {
                new_line = false;
                let gap = x - self.last_end;
                let ends_space = cur.cells.last().map(|c| c.text.ends_with(' ')).unwrap_or(true);
                if gap > 0.2 * size && !ends_space && !g.text.starts_with(' ') {
                    cur.cells.push(Cell { text: " ".into(), x: self.last_end, w: gap });
                }
                cur.cells.push(Cell { text: g.text.to_string(), x, w });
                cur.size = cur.size.max(size);
            }
        }
        if new_line {
            self.lines.push(TextLine { cells: vec![Cell { text: g.text.to_string(), x, w }], y: y0, size });
        }
        self.last_end = x + w;
    }
}

/// The text of a page: lines joined with newlines, a blank line where the vertical gap is large.
pub fn page_text(lines: &[TextLine]) -> String {
    let mut out = String::new();
    let mut prev: Option<&TextLine> = None;
    for l in lines {
        let t = l.text();
        let t = t.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        if let Some(p) = prev {
            out.push('\n');
            if p.y - l.y > 1.8 * l.size.max(p.size) && p.y - l.y < 1e6 {
                out.push('\n');
            }
        }
        out.push_str(t);
        prev = Some(l);
    }
    out
}

/// A search hit on one page: the boxes (user space x0, y0, x1, y1) of the matched text, one per line it touches.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub rects: Vec<[f64; 4]>,
    pub context: String,
}

fn fold(c: char, case_sensitive: bool) -> char {
    if case_sensitive {
        c
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

pub fn find(lines: &[TextLine], query: &str, case_sensitive: bool) -> Vec<Hit> {
    let q: Vec<char> = query.chars().map(|c| fold(c, case_sensitive)).collect();
    if q.is_empty() || q.len() > 200 {
        return vec![];
    }
    let mut hits = Vec::new();
    for l in lines {
        let mut chars: Vec<(char, usize)> = Vec::new();
        for (ci, c) in l.cells.iter().enumerate() {
            for ch in c.text.chars() {
                chars.push((fold(ch, case_sensitive), ci));
            }
        }
        if chars.len() < q.len() {
            continue;
        }
        let mut i = 0;
        while i + q.len() <= chars.len() {
            if chars[i..i + q.len()].iter().map(|c| c.0).eq(q.iter().copied()) {
                let (a, b) = (chars[i].1, chars[i + q.len() - 1].1);
                let x0 = l.cells[a].x;
                let x1 = l.cells[b].x + l.cells[b].w;
                let rect = [x0, l.y - 0.22 * l.size, x1.max(x0), l.y + 0.82 * l.size];
                let text = l.text();
                let chars_all: Vec<char> = text.chars().collect();
                let from = i.saturating_sub(24);
                let to = (i + q.len() + 24).min(chars_all.len());
                let context: String = chars_all[from.min(chars_all.len())..to.max(from.min(chars_all.len()))].iter().collect();
                hits.push(Hit { rects: vec![rect], context });
                i += q.len();
                if hits.len() >= 2000 {
                    return hits;
                }
            } else {
                i += 1;
            }
        }
    }
    hits
}
