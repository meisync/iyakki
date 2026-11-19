//! Table analysis for the lines of a page (F-PDF-18): rows that share column positions are found and given back as rows of cells,
//! and can be written as a Markdown table. Plain geometry on the lines the interpreter produced; no model, nothing is run.
//!
//! A line is cut into fields where it has a wide gap (`FIELD_GAP` font sizes). Lines that have at least two fields and follow each
//! other closely form a block; the fields of a block are projected on the x axis, the strips nobody covers (`COLUMN_GAP` font
//! sizes or wider) are the gutters between columns. A block is a table when it has at least `MIN_ROWS` rows and two columns and most
//! rows fill at least two columns. A field that reaches across a gutter removes that gutter (a spanning heading is not a table).
//! Right-aligned numbers work because a field belongs to the column whose strip it overlaps most, not to a left edge.

use crate::text::{Cell, TextLine};

const FIELD_GAP: f64 = 1.2;
const COLUMN_GAP: f64 = 0.8;
const MIN_ROWS: usize = 3;
/// Two rows further apart than this many font sizes are not neighbours.
const ROW_GAP: f64 = 3.2;
pub const MAX_LINES: usize = 20_000;
pub const MAX_COLUMNS: usize = 40;

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// Rows of cells, top to bottom; a cell with nothing in it is an empty string.
    pub rows: Vec<Vec<String>>,
    /// The range of the page's lines (indexes into the slice given) the table covers, last one included.
    pub first_line: usize,
    pub last_line: usize,
    /// The left and right edge of the table in user space.
    pub x0: f64,
    pub x1: f64,
}

#[derive(Clone)]
struct Field {
    text: String,
    x0: f64,
    x1: f64,
}

fn fields(l: &TextLine) -> Vec<Field> {
    let mut out: Vec<Field> = Vec::new();
    let mut cur: Vec<&Cell> = Vec::new();
    let flush = |cur: &mut Vec<&Cell>, out: &mut Vec<Field>| {
        let text: String = cur.iter().map(|c| c.text.as_str()).collect::<String>().trim().to_string();
        if !text.is_empty() {
            let x0 = cur.iter().map(|c| c.x).fold(f64::MAX, f64::min);
            let x1 = cur.iter().map(|c| c.x + c.w.max(0.0)).fold(f64::MIN, f64::max);
            out.push(Field { text, x0, x1 });
        }
        cur.clear();
    };
    for c in &l.cells {
        if c.text.trim().is_empty() && c.w > FIELD_GAP * l.size {
            flush(&mut cur, &mut out);
        } else {
            cur.push(c);
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn finite(l: &TextLine) -> bool {
    l.y.is_finite() && l.size.is_finite() && l.size > 0.0 && l.cells.iter().all(|c| c.x.is_finite() && c.w.is_finite())
}

/// The column strips of a block: from the left edge, each strip is (x0, x1) of the text it holds.
fn columns(rows: &[Vec<Field>], size: f64) -> Vec<(f64, f64)> {
    let mut cover: Vec<(f64, f64)> = rows.iter().flatten().map(|f| (f.x0, f.x1.max(f.x0))).collect();
    cover.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cols: Vec<(f64, f64)> = Vec::new();
    for (a, b) in cover {
        match cols.last_mut() {
            Some(c) if a - c.1 < COLUMN_GAP * size => c.1 = c.1.max(b),
            _ => cols.push((a, b)),
        }
    }
    cols
}

fn column_of(cols: &[(f64, f64)], f: &Field) -> usize {
    let mut best = 0;
    let mut best_overlap = f64::MIN;
    for (i, c) in cols.iter().enumerate() {
        let overlap = f.x1.min(c.1) - f.x0.max(c.0);
        if overlap > best_overlap {
            best_overlap = overlap;
            best = i;
        }
    }
    best
}

/// The tables of a page's lines (lines in reading order, as `Document::page_lines` gives them). Nothing found gives an empty list.
pub fn find_tables(lines: &[TextLine]) -> Vec<Table> {
    let mut out = Vec::new();
    if lines.len() < MIN_ROWS || lines.len() > MAX_LINES || !lines.iter().all(finite) {
        return out;
    }
    let all: Vec<Vec<Field>> = lines.iter().map(fields).collect();
    let mut i = 0;
    while i < lines.len() {
        if all[i].len() < 2 {
            i += 1;
            continue;
        }
        // a block of neighbouring lines with two or more fields each
        let mut j = i;
        while j + 1 < lines.len() && all[j + 1].len() >= 2 {
            let gap = lines[j].y - lines[j + 1].y;
            let size = lines[j].size.max(lines[j + 1].size);
            if gap <= 0.0 || gap > ROW_GAP * size {
                break;
            }
            j += 1;
        }
        if j + 1 - i >= MIN_ROWS {
            let size = lines[i..=j].iter().map(|l| l.size).fold(0.0, f64::max);
            let block = &all[i..=j];
            let cols = columns(block, size);
            if cols.len() >= 2 && cols.len() <= MAX_COLUMNS {
                let mut rows: Vec<Vec<String>> = Vec::new();
                let mut multi = 0usize;
                for r in block {
                    let mut cells = vec![String::new(); cols.len()];
                    let mut filled = 0;
                    for f in r {
                        let c = column_of(&cols, f);
                        if cells[c].is_empty() {
                            filled += 1;
                        } else {
                            cells[c].push(' ');
                        }
                        cells[c].push_str(&f.text);
                    }
                    if filled >= 2 {
                        multi += 1;
                    }
                    rows.push(cells);
                }
                // most rows must stand in two or more columns, and no column may be empty
                let used = (0..cols.len()).all(|c| rows.iter().any(|r| !r[c].is_empty()));
                if multi * 3 >= rows.len() * 2 && used {
                    out.push(Table { rows, first_line: i, last_line: j, x0: cols[0].0, x1: cols[cols.len() - 1].1 });
                }
            }
        }
        i = j + 1;
    }
    out
}

fn cell_md(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// The table as Markdown; the first row is the header.
pub fn table_markdown(t: &Table) -> String {
    let n = t.rows.first().map_or(0, Vec::len);
    if n == 0 {
        return String::new();
    }
    let mut out = String::new();
    for (k, r) in t.rows.iter().enumerate() {
        out.push_str("| ");
        out.push_str(&r.iter().map(|c| cell_md(c)).collect::<Vec<_>>().join(" | "));
        out.push_str(" |\n");
        if k == 0 {
            out.push('|');
            out.push_str(&" --- |".repeat(n));
            out.push('\n');
        }
    }
    out
}

/// The text of a page where each table is written as a Markdown table and everything else as `page_text` writes it.
pub fn page_text_with_tables(lines: &[TextLine]) -> String {
    let tables = find_tables(lines);
    if tables.is_empty() {
        return crate::text::page_text(lines);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut at = 0;
    for t in &tables {
        if t.first_line > at {
            parts.push(crate::text::page_text(&lines[at..t.first_line]));
        }
        parts.push(table_markdown(t).trim_end().to_string());
        at = t.last_line + 1;
    }
    if at < lines.len() {
        parts.push(crate::text::page_text(&lines[at..]));
    }
    parts.retain(|p| !p.trim().is_empty());
    parts.join("\n\n")
}
