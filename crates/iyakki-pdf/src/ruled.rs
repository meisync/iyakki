//! Ruled tables (F-PDF-18): a grid drawn with lines or thin rectangles gives the cell borders, and the text lines are put into the
//! cells they fall in. A cell may hold several lines (they are joined with a space, top to bottom). Plain geometry; no model.
//!
//! `RuleSink` collects the horizontal and vertical rules of a page from strokes and from thin filled rectangles. Rules that touch each
//! other form one component; a component with at least two columns and two rows of cells is a grid. A line of text belongs to a grid when
//! all of its cells lie inside the grid's box.

use crate::geom::Seg;
use crate::interp::{GState, Sink};
use crate::table::Table;
use crate::text::TextLine;

/// A rule shorter than this is a dot, not a border.
const MIN_RULE: f64 = 4.0;
/// A filled rectangle thinner than this is a rule.
const THIN: f64 = 2.5;
/// Two coordinates this close are the same border; rules this close touch.
const TOL: f64 = 2.0;
pub const MAX_RULES: usize = 6000;
pub const MAX_GRID_LINES: usize = 80;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    /// A horizontal rule has `a` = x start, `b` = x end and `at` = y; a vertical one has `a` = y start, `b` = y end and `at` = x.
    pub at: f64,
    pub a: f64,
    pub b: f64,
    pub horizontal: bool,
}

#[derive(Default)]
pub struct RuleSink {
    pub rules: Vec<Rule>,
}

impl RuleSink {
    fn add(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        if self.rules.len() >= MAX_RULES || ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
            return;
        }
        let (dx, dy) = ((x1 - x0).abs(), (y1 - y0).abs());
        if dy <= 0.5 && dx >= MIN_RULE {
            self.rules.push(Rule { at: (y0 + y1) / 2.0, a: x0.min(x1), b: x0.max(x1), horizontal: true });
        } else if dx <= 0.5 && dy >= MIN_RULE {
            self.rules.push(Rule { at: (x0 + x1) / 2.0, a: y0.min(y1), b: y0.max(y1), horizontal: false });
        }
    }
}

impl Sink for RuleSink {
    fn stroke(&mut self, path: &[Seg], _gs: &GState) {
        let mut start = (0.0, 0.0);
        let mut cur = (0.0, 0.0);
        for s in path {
            match *s {
                Seg::Move(x, y) => {
                    start = (x, y);
                    cur = (x, y);
                }
                Seg::Line(x, y) => {
                    self.add(cur.0, cur.1, x, y);
                    cur = (x, y);
                }
                Seg::Close => {
                    self.add(cur.0, cur.1, start.0, start.1);
                    cur = start;
                }
                Seg::Curve(_, _, _, _, x, y) => cur = (x, y),
            }
        }
    }

    fn fill(&mut self, path: &[Seg], _even_odd: bool, _gs: &GState) {
        // a rectangle (move, three lines, close) that is thin is a rule drawn as a fill
        let mut pts: Vec<(f64, f64)> = Vec::new();
        for s in path {
            match *s {
                Seg::Move(x, y) | Seg::Line(x, y) => pts.push((x, y)),
                Seg::Close => {}
                Seg::Curve(..) => return,
            }
        }
        if pts.len() != 4 || !pts.iter().all(|p| p.0.is_finite() && p.1.is_finite()) {
            return;
        }
        let x0 = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        let x1 = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
        let y0 = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
        let y1 = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
        let (w, h) = (x1 - x0, y1 - y0);
        if h <= THIN && w >= MIN_RULE {
            self.add(x0, (y0 + y1) / 2.0, x1, (y0 + y1) / 2.0);
        } else if w <= THIN && h >= MIN_RULE {
            self.add((x0 + x1) / 2.0, y0, (x0 + x1) / 2.0, y1);
        }
    }
}

/// A grid: the border positions, left to right and top to bottom (y falls).
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
}

fn touches(h: &Rule, v: &Rule) -> bool {
    v.at >= h.a - TOL && v.at <= h.b + TOL && h.at >= v.a - TOL && h.at <= v.b + TOL
}

fn find_root(p: &mut [usize], mut i: usize) -> usize {
    while p[i] != i {
        p[i] = p[p[i]];
        i = p[i];
    }
    i
}

fn levels(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(|a, b| a.total_cmp(b));
    let mut out: Vec<f64> = Vec::new();
    for x in v {
        match out.last() {
            Some(l) if x - l <= TOL => {}
            _ => out.push(x),
        }
    }
    out
}

/// The grids of a page's rules.
pub fn find_grids(rules: &[Rule]) -> Vec<Grid> {
    if rules.len() > MAX_RULES {
        return Vec::new();
    }
    let n = rules.len();
    let mut parent: Vec<usize> = (0..n).collect();
    let hs: Vec<usize> = (0..n).filter(|&i| rules[i].horizontal).collect();
    let vs: Vec<usize> = (0..n).filter(|&i| !rules[i].horizontal).collect();
    for &h in &hs {
        for &v in &vs {
            if touches(&rules[h], &rules[v]) {
                let (a, b) = (find_root(&mut parent, h), find_root(&mut parent, v));
                parent[a] = b;
            }
        }
    }
    let mut roots: Vec<usize> = Vec::new();
    for i in 0..n {
        let r = find_root(&mut parent, i);
        if !roots.contains(&r) {
            roots.push(r);
        }
    }
    let mut out = Vec::new();
    for r in roots {
        let members: Vec<usize> = (0..n).filter(|&i| find_root(&mut parent, i) == r).collect();
        let xs = levels(members.iter().filter(|&&i| !rules[i].horizontal).map(|&i| rules[i].at).collect());
        let mut ys = levels(members.iter().filter(|&&i| rules[i].horizontal).map(|&i| rules[i].at).collect());
        if xs.len() >= 3 && ys.len() >= 3 && xs.len() <= MAX_GRID_LINES && ys.len() <= MAX_GRID_LINES {
            ys.reverse();
            out.push(Grid { xs, ys });
        }
    }
    // reading order: top first
    out.sort_by(|a, b| b.ys[0].total_cmp(&a.ys[0]));
    out
}

fn inside(g: &Grid, l: &TextLine) -> bool {
    let (x0, x1) = (g.xs[0] - TOL, g.xs[g.xs.len() - 1] + TOL);
    let (top, bottom) = (g.ys[0] + TOL, g.ys[g.ys.len() - 1] - TOL);
    !l.cells.is_empty() && l.y <= top && l.y >= bottom && l.cells.iter().all(|c| c.x + c.w / 2.0 >= x0 && c.x + c.w / 2.0 <= x1)
}

/// Which lines (indexes) a grid holds.
pub fn lines_in(g: &Grid, lines: &[TextLine]) -> Vec<usize> {
    (0..lines.len()).filter(|&i| inside(g, &lines[i])).collect()
}

/// The table a grid makes from the lines it holds. Text goes to the cell its middle falls in; the lines of a cell are joined by a space.
pub fn grid_table(g: &Grid, lines: &[TextLine], held: &[usize]) -> Table {
    let (nc, nr) = (g.xs.len() - 1, g.ys.len() - 1);
    let mut cells = vec![vec![String::new(); nc]; nr];
    let mut order: Vec<usize> = held.to_vec();
    order.sort_by(|&a, &b| lines[b].y.total_cmp(&lines[a].y));
    for &li in &order {
        let l = &lines[li];
        // the row whose borders the baseline lies between (a baseline sits a little above the lower border)
        let row = (0..nr).find(|&r| l.y <= g.ys[r] + TOL && l.y >= g.ys[r + 1] - TOL);
        let Some(row) = row else { continue };
        let mut per_col: Vec<String> = vec![String::new(); nc];
        for c in &l.cells {
            let mid = c.x + c.w / 2.0;
            let col = (0..nc).find(|&k| mid >= g.xs[k] - TOL && mid <= g.xs[k + 1] + TOL).unwrap_or(0);
            per_col[col].push_str(&c.text);
        }
        for (k, t) in per_col.iter().enumerate() {
            let t = t.trim();
            if t.is_empty() {
                continue;
            }
            if !cells[row][k].is_empty() {
                cells[row][k].push(' ');
            }
            cells[row][k].push_str(t);
        }
    }
    Table {
        rows: cells,
        first_line: order.last().copied().unwrap_or(0),
        last_line: order.first().copied().unwrap_or(0),
        x0: g.xs[0],
        x1: g.xs[g.xs.len() - 1],
    }
}

/// The text of a page: ruled tables as Markdown tables where they stand, other tables found by their gutters, everything else as text.
pub fn page_text_with_rules(lines: &[TextLine], rules: &[Rule]) -> String {
    let grids = if lines.len() > crate::table::MAX_LINES { Vec::new() } else { find_grids(rules) };
    let mut taken = vec![usize::MAX; lines.len()];
    let mut tables: Vec<Table> = Vec::new();
    for g in &grids {
        let held: Vec<usize> = lines_in(g, lines).into_iter().filter(|&i| taken[i] == usize::MAX).collect();
        if held.is_empty() {
            continue;
        }
        let t = grid_table(g, lines, &held);
        // a grid with no cell that has text is a drawing, not a table
        if t.rows.iter().all(|r| r.iter().all(String::is_empty)) {
            continue;
        }
        for &i in &held {
            taken[i] = tables.len();
        }
        tables.push(t);
    }
    if tables.is_empty() {
        return crate::table::page_text_with_tables(lines);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut run: Vec<TextLine> = Vec::new();
    let mut emitted = vec![false; tables.len()];
    let flush = |run: &mut Vec<TextLine>, parts: &mut Vec<String>| {
        if !run.is_empty() {
            parts.push(crate::table::page_text_with_tables(run));
            run.clear();
        }
    };
    for (i, l) in lines.iter().enumerate() {
        match taken[i] {
            usize::MAX => run.push(l.clone()),
            k => {
                if !emitted[k] {
                    flush(&mut run, &mut parts);
                    parts.push(crate::table::table_markdown(&tables[k]).trim_end().to_string());
                    emitted[k] = true;
                }
            }
        }
    }
    flush(&mut run, &mut parts);
    parts.retain(|p| !p.trim().is_empty());
    parts.join("\n\n")
}
