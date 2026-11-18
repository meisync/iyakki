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

fn is_rtl_char(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x05FF | 0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Text that was drawn right to left comes out of a content stream in visual order (the first letter on the left); this gives it back in
/// the order it is read and typed. A line with no right-to-left letter is returned as it is.
pub fn visual_to_logical(line: &str) -> String {
    if !line.chars().any(is_rtl_char) {
        return line.to_string();
    }
    // the reordering of the bidirectional algorithm is its own inverse for a line whose base direction is the same both ways
    let info = unicode_bidi::BidiInfo::new(line, Some(unicode_bidi::Level::rtl()));
    let mut out = String::with_capacity(line.len());
    for p in &info.paragraphs {
        out.push_str(&info.reorder_line(p, p.range.clone()));
    }
    out
}

/// The text of a page: lines joined with newlines, a blank line where the vertical gap is large.
pub fn page_text(lines: &[TextLine]) -> String {
    let mut out = String::new();
    let mut prev: Option<&TextLine> = None;
    for l in lines {
        let t = visual_to_logical(&l.text());
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

/// One character of a page with its box (user space x0, y0, x1, y1) and the line it is on: what text selection works on.
#[derive(Clone, Debug, PartialEq)]
pub struct CharBox {
    pub ch: char,
    pub rect: [f64; 4],
    pub line: usize,
}

/// The characters of a page in reading order. A cell of several characters shares its width between them.
pub fn char_boxes(lines: &[TextLine]) -> Vec<CharBox> {
    let mut out = Vec::new();
    for (li, l) in lines.iter().enumerate() {
        for c in &l.cells {
            let n = c.text.chars().count().max(1);
            let w = c.w / n as f64;
            for (k, ch) in c.text.chars().enumerate() {
                let x0 = c.x + w * k as f64;
                out.push(CharBox { ch, rect: [x0, l.y - 0.22 * l.size, x0 + w, l.y + 0.82 * l.size], line: li });
                if out.len() >= 200_000 {
                    return out;
                }
            }
        }
    }
    out
}

/// The character nearest to a point (user space): the closest line vertically, then the closest box along it. `None` for an empty page.
pub fn char_at(boxes: &[CharBox], x: f64, y: f64) -> Option<usize> {
    let dist = |r: &[f64; 4]| {
        let dy = if y < r[1] { r[1] - y } else if y > r[3] { y - r[3] } else { 0.0 };
        let dx = if x < r[0] { r[0] - x } else if x > r[2] { x - r[2] } else { 0.0 };
        (dy, dx)
    };
    let mut best: Option<(usize, f64, f64)> = None;
    for (i, b) in boxes.iter().enumerate() {
        let (dy, dx) = dist(&b.rect);
        if best.is_none_or(|(_, by, bx)| dy < by - 1e-9 || ((dy - by).abs() <= 1e-9 && dx < bx)) {
            best = Some((i, dy, dx));
        }
    }
    best.map(|b| b.0)
}

/// The word around character `i` (letters and digits), as an inclusive range.
pub fn word_range(boxes: &[CharBox], i: usize) -> (usize, usize) {
    let w = |k: usize| boxes[k].ch.is_alphanumeric();
    if i >= boxes.len() || !w(i) {
        return (i, i);
    }
    let (mut a, mut b) = (i, i);
    while a > 0 && w(a - 1) && boxes[a - 1].line == boxes[i].line {
        a -= 1;
    }
    while b + 1 < boxes.len() && w(b + 1) && boxes[b + 1].line == boxes[i].line {
        b += 1;
    }
    (a, b)
}

/// The whole line character `i` is on, as an inclusive range.
pub fn line_range(boxes: &[CharBox], i: usize) -> (usize, usize) {
    if i >= boxes.len() {
        return (i, i);
    }
    let l = boxes[i].line;
    let a = boxes.iter().position(|b| b.line == l).unwrap_or(i);
    let b = boxes.iter().rposition(|b| b.line == l).unwrap_or(i);
    (a, b)
}

/// The text between two characters (either order, both included); a new line where the line changes.
pub fn selection_text(boxes: &[CharBox], a: usize, b: usize) -> String {
    if boxes.is_empty() {
        return String::new();
    }
    let (a, b) = (a.min(b), a.max(b).min(boxes.len() - 1));
    let mut out = String::new();
    for k in a..=b {
        if k > a && boxes[k].line != boxes[k - 1].line {
            out.push('\n');
        }
        out.push(boxes[k].ch);
    }
    out.trim_end().to_string()
}

/// One box per line the selection touches.
pub fn selection_rects(boxes: &[CharBox], a: usize, b: usize) -> Vec<[f64; 4]> {
    if boxes.is_empty() {
        return Vec::new();
    }
    let (a, b) = (a.min(b), a.max(b).min(boxes.len() - 1));
    let mut out: Vec<[f64; 4]> = Vec::new();
    let mut line = usize::MAX;
    for k in a..=b {
        let r = boxes[k].rect;
        if boxes[k].line == line {
            if let Some(last) = out.last_mut() {
                last[0] = last[0].min(r[0]);
                last[1] = last[1].min(r[1]);
                last[2] = last[2].max(r[2]);
                last[3] = last[3].max(r[3]);
            }
        } else {
            line = boxes[k].line;
            out.push(r);
        }
    }
    out
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
