//! The expression stage of a panel: small lines of text that combine the frames of the queries (A, B, C ...) into new series
//! or a joined table. One line, one result:
//!
//! ```text
//! C = A / B * 100              math between series (+ - * / and numbers, parentheses, rate(X), abs(X))
//! D = merge A, B               all the series of both
//! T = join A, B on key         table join: rows of A and B with the same text in column `key`
//! E = top 5 A                  the 5 series with the highest average
//! F = filter A where host=web1 keep series whose label matches (= or !=)
//! G = group A by host sum      sum, avg, max or min of series sharing a label (series at identical times)
//! R = rename A as "{{host}}"   new legend from labels
//! ```
//!
//! Time alignment for math (`Align`): `nearest` (the other series' closest point within the tolerance), `previous` (its last
//! point at or before) and `interpolate` (a straight line between its neighbours); points with no partner inside the
//! tolerance are dropped, and so are results that are not numbers (a division by zero).

use std::collections::BTreeMap;

use super::frame::*;
use super::model::series_label;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Nearest,
    Previous,
    Interpolate,
}

impl Align {
    pub fn parse(s: &str) -> Option<Align> {
        match s.trim().to_ascii_lowercase().as_str() {
            "nearest" | "" => Some(Align::Nearest),
            "previous" | "prev" | "last" => Some(Align::Previous),
            "interpolate" | "linear" => Some(Align::Interpolate),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Opts {
    pub align: Align,
    /// How far (ms) a partner point may be; 0 picks twice the typical spacing of the partner.
    pub tolerance_ms: i64,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { align: Align::Nearest, tolerance_ms: 0 }
    }
}

#[derive(Clone, Debug)]
pub enum Val {
    Num(f64),
    Set(Vec<Series>),
    Table(Frame),
}

#[derive(Debug, Default)]
pub struct Output {
    pub series: Vec<Series>,
    pub table: Option<Frame>,
    /// One sentence per line that failed.
    pub errors: Vec<String>,
}

// ------------------------------------------------------------------------------------------------------ math on series

fn spacing(points: &[(i64, f64)]) -> i64 {
    if points.len() < 2 {
        return 60_000;
    }
    let mut d: Vec<i64> = points.windows(2).map(|w| (w[1].0 - w[0].0).max(1)).collect();
    d.sort_unstable();
    d[d.len() / 2]
}

/// The value of `points` (sorted by time) at `t`, or `None` when nothing is near enough.
pub fn value_at(points: &[(i64, f64)], t: i64, align: Align, tol: i64) -> Option<f64> {
    if points.is_empty() {
        return None;
    }
    let tol = if tol > 0 { tol } else { 2 * spacing(points) };
    let i = points.partition_point(|p| p.0 < t);
    let prev = if i > 0 { Some(points[i - 1]) } else { None };
    let next = points.get(i).copied();
    match align {
        Align::Nearest => {
            let cand = match (prev, next) {
                (Some(a), Some(b)) => if (t - a.0).abs() <= (b.0 - t).abs() { a } else { b },
                (Some(a), None) => a,
                (None, Some(b)) => b,
                _ => return None,
            };
            ((cand.0 - t).abs() <= tol).then_some(cand.1)
        }
        Align::Previous => {
            let cand = if next.is_some_and(|n| n.0 == t) { next } else { prev };
            cand.filter(|c| t - c.0 <= tol).map(|c| c.1)
        }
        Align::Interpolate => match (prev, next) {
            (_, Some(b)) if b.0 == t => Some(b.1),
            (Some(a), Some(b)) if b.0 - a.0 <= tol * 2 => Some(a.1 + (b.1 - a.1) * ((t - a.0) as f64 / (b.0 - a.0) as f64)),
            (Some(a), None) if t - a.0 <= tol => Some(a.1),
            (None, Some(b)) if b.0 - t <= tol => Some(b.1),
            _ => None,
        },
    }
}

fn clean(l: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    l.iter().filter(|(k, _)| *k != "__name__").map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn apply_op(op: char, a: f64, b: f64) -> Option<f64> {
    let v = match op {
        '+' => a + b,
        '-' => a - b,
        '*' => a * b,
        '/' => {
            if b == 0.0 {
                return None;
            }
            a / b
        }
        _ => return None,
    };
    v.is_finite().then_some(v)
}

fn binary(op: char, l: Val, r: Val, o: &Opts) -> Result<Val, String> {
    match (l, r) {
        (Val::Num(a), Val::Num(b)) => apply_op(op, a, b).map(Val::Num).ok_or_else(|| "the result is not a number".to_string()),
        (Val::Set(s), Val::Num(b)) => Ok(Val::Set(s.into_iter().map(|mut x| {
            x.points = x.points.iter().filter_map(|p| apply_op(op, p.1, b).map(|v| (p.0, v))).collect();
            x
        }).collect())),
        (Val::Num(a), Val::Set(s)) => Ok(Val::Set(s.into_iter().map(|mut x| {
            x.points = x.points.iter().filter_map(|p| apply_op(op, a, p.1).map(|v| (p.0, v))).collect();
            x
        }).collect())),
        (Val::Set(a), Val::Set(b)) => {
            let mut out = vec![];
            for (i, x) in a.iter().enumerate() {
                // the partner: the only series of the other side, the one with the same labels, or the one at the same position
                let partner = if b.len() == 1 {
                    b.first()
                } else {
                    b.iter().find(|y| clean(&y.labels) == clean(&x.labels) && !x.labels.is_empty()).or_else(|| if a.len() == b.len() { b.get(i) } else { None })
                };
                let Some(y) = partner else { continue };
                let pts: Vec<(i64, f64)> = x.points.iter().filter_map(|p| value_at(&y.points, p.0, o.align, o.tolerance_ms).and_then(|v| apply_op(op, p.1, v)).map(|v| (p.0, v))).collect();
                out.push(Series { name: x.name.clone(), labels: x.labels.clone(), points: pts, axis: x.axis });
            }
            if out.is_empty() && !a.is_empty() && !b.is_empty() {
                return Err("no series of the two sides belong together (their labels differ); use filter, group or merge first".into());
            }
            Ok(Val::Set(out))
        }
        _ => Err("a table cannot be used in math".into()),
    }
}

fn rate(s: Vec<Series>) -> Vec<Series> {
    s.into_iter()
        .map(|mut x| {
            x.points = x.points.windows(2).filter_map(|w| {
                let dt = (w[1].0 - w[0].0) as f64 / 1000.0;
                (dt > 0.0).then(|| (w[1].0, ((w[1].1 - w[0].1) / dt).max(f64::MIN))).filter(|p| p.1.is_finite())
            }).collect();
            x
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------------ parsing math

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Id(String),
    Op(char),
    LP,
    RP,
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = s.chars().collect();
    let (mut i, mut out) = (0, vec![]);
    while i < c.len() {
        match c[i] {
            ch if ch.is_whitespace() => i += 1,
            '+' | '-' | '*' | '/' => {
                out.push(Tok::Op(c[i]));
                i += 1;
            }
            '(' => {
                out.push(Tok::LP);
                i += 1;
            }
            ')' => {
                out.push(Tok::RP);
                i += 1;
            }
            ch if ch.is_ascii_digit() || ch == '.' => {
                let st = i;
                while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                    i += 1;
                }
                let t: String = c[st..i].iter().collect();
                out.push(Tok::Num(t.parse().map_err(|_| format!("`{t}` is not a number"))?));
            }
            ch if ch.is_alphabetic() || ch == '_' => {
                let st = i;
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                    i += 1;
                }
                out.push(Tok::Id(c[st..i].iter().collect()));
            }
            other => return Err(format!("unexpected `{other}`")),
        }
    }
    Ok(out)
}

struct P<'a> {
    t: Vec<Tok>,
    i: usize,
    env: &'a BTreeMap<String, Val>,
    o: &'a Opts,
}

impl P<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn expr(&mut self) -> Result<Val, String> {
        let mut l = self.term()?;
        while let Some(Tok::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.i += 1;
            let r = self.term()?;
            l = binary(op, l, r, self.o)?;
        }
        Ok(l)
    }
    fn term(&mut self) -> Result<Val, String> {
        let mut l = self.atom()?;
        while let Some(Tok::Op(op @ ('*' | '/'))) = self.peek().cloned() {
            self.i += 1;
            let r = self.atom()?;
            l = binary(op, l, r, self.o)?;
        }
        Ok(l)
    }
    fn atom(&mut self) -> Result<Val, String> {
        match self.t.get(self.i).cloned() {
            Some(Tok::Num(n)) => {
                self.i += 1;
                Ok(Val::Num(n))
            }
            Some(Tok::Op('-')) => {
                self.i += 1;
                let v = self.atom()?;
                binary('*', v, Val::Num(-1.0), self.o)
            }
            Some(Tok::LP) => {
                self.i += 1;
                let v = self.expr()?;
                if self.peek() != Some(&Tok::RP) {
                    return Err("a `)` is missing".into());
                }
                self.i += 1;
                Ok(v)
            }
            Some(Tok::Id(name)) => {
                self.i += 1;
                if self.peek() == Some(&Tok::LP) && matches!(name.as_str(), "rate" | "abs") {
                    self.i += 1;
                    let v = self.expr()?;
                    if self.peek() != Some(&Tok::RP) {
                        return Err("a `)` is missing".into());
                    }
                    self.i += 1;
                    return match (name.as_str(), v) {
                        ("rate", Val::Set(s)) => Ok(Val::Set(rate(s))),
                        ("abs", Val::Set(s)) => Ok(Val::Set(s.into_iter().map(|mut x| {
                            x.points.iter_mut().for_each(|p| p.1 = p.1.abs());
                            x
                        }).collect())),
                        ("abs", Val::Num(n)) => Ok(Val::Num(n.abs())),
                        (f, _) => Err(format!("{f}() needs a query or an expression result")),
                    };
                }
                self.env.get(&name).cloned().ok_or_else(|| format!("`{name}` is not a query or an earlier result"))
            }
            Some(t) => Err(format!("unexpected {t:?}")),
            None => Err("the expression ends too soon".into()),
        }
    }
}

fn eval_math(rhs: &str, env: &BTreeMap<String, Val>, o: &Opts) -> Result<Val, String> {
    let t = lex(rhs)?;
    let mut p = P { t, i: 0, env, o };
    let v = p.expr()?;
    if p.i != p.t.len() {
        return Err("something is left over after the expression".into());
    }
    Ok(v)
}

// ------------------------------------------------------------------------------------------------------ other operations

fn set_of(env: &BTreeMap<String, Val>, name: &str) -> Result<Vec<Series>, String> {
    match env.get(name.trim()) {
        Some(Val::Set(s)) => Ok(s.clone()),
        Some(_) => Err(format!("`{}` is not a set of series", name.trim())),
        None => Err(format!("`{}` is not a query or an earlier result", name.trim())),
    }
}

fn table_of(env: &BTreeMap<String, Val>, name: &str) -> Result<Frame, String> {
    match env.get(name.trim()) {
        Some(Val::Table(f)) => Ok(f.clone()),
        Some(Val::Set(s)) => Ok(Frame { series: s.clone(), ..Default::default() }.series_table()),
        _ => Err(format!("`{}` has no table", name.trim())),
    }
}

fn join(a: &Frame, b: &Frame, key: &str) -> Result<Frame, String> {
    let (Some(ka), Some(kb)) = (a.column_index(key), b.column_index(key)) else {
        return Err(format!("both tables need a column `{key}` (A has: {}; B has: {})", names(a), names(b)));
    };
    let mut columns = a.columns.clone();
    let b_cols: Vec<usize> = (0..b.columns.len()).filter(|i| *i != kb).collect();
    for &i in &b_cols {
        let mut c = b.columns[i].clone();
        if columns.iter().any(|x| x.name == c.name) {
            c.name = format!("{}_2", c.name);
        }
        columns.push(c);
    }
    let mut index: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for r in 0..b.rows.len() {
        index.entry(b.cell(r, kb)).or_default().push(r);
    }
    let mut rows = vec![];
    for r in 0..a.rows.len() {
        let Some(hits) = index.get(&a.cell(r, ka)) else { continue };
        for &h in hits {
            let mut row = a.rows[r].clone();
            row.extend(b_cols.iter().map(|&i| b.rows[h].get(i).cloned().unwrap_or(serde_json::Value::Null)));
            rows.push(row);
        }
    }
    Ok(Frame { columns, rows, ..Default::default() })
}

fn names(f: &Frame) -> String {
    f.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    t.strip_prefix('"').and_then(|x| x.strip_suffix('"')).or_else(|| t.strip_prefix('\'').and_then(|x| x.strip_suffix('\''))).unwrap_or(t).to_string()
}

fn group(s: Vec<Series>, label: &str, func: &str) -> Result<Vec<Series>, String> {
    if !matches!(func, "sum" | "avg" | "max" | "min") {
        return Err(format!("`{func}` is not sum, avg, max or min"));
    }
    let mut groups: BTreeMap<String, Vec<Series>> = BTreeMap::new();
    for x in s {
        groups.entry(x.labels.get(label).cloned().unwrap_or_default()).or_default().push(x);
    }
    let mut out = vec![];
    for (k, members) in groups {
        let mut at: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
        for m in &members {
            for p in &m.points {
                at.entry(p.0).or_default().push(p.1);
            }
        }
        let pts = at
            .into_iter()
            .map(|(t, v)| {
                let n = v.len() as f64;
                let r = match func {
                    "sum" => v.iter().sum(),
                    "avg" => v.iter().sum::<f64>() / n,
                    "max" => v.iter().cloned().fold(f64::MIN, f64::max),
                    _ => v.iter().cloned().fold(f64::MAX, f64::min),
                };
                (t, r)
            })
            .collect();
        out.push(Series { name: if k.is_empty() { func.to_string() } else { k.clone() }, labels: [(label.to_string(), k)].into(), points: pts, axis: members[0].axis });
    }
    Ok(out)
}

fn eval_line(rhs: &str, env: &BTreeMap<String, Val>, o: &Opts) -> Result<Val, String> {
    let rhs = rhs.trim();
    let (word, rest) = rhs.split_once(char::is_whitespace).unwrap_or((rhs, ""));
    match word {
        "merge" => {
            let mut all = vec![];
            for n in rest.split(',') {
                all.extend(set_of(env, n)?);
            }
            Ok(Val::Set(all))
        }
        "join" => {
            let (pair, key) = rest.split_once(" on ").ok_or("write: join A, B on column")?;
            let mut it = pair.split(',');
            let (a, b) = (it.next().ok_or("join needs two queries")?, it.next().ok_or("join needs two queries")?);
            Ok(Val::Table(join(&table_of(env, a)?, &table_of(env, b)?, key.trim())?))
        }
        "top" => {
            let (n, r) = rest.trim().split_once(char::is_whitespace).ok_or("write: top 5 A")?;
            let n: usize = n.parse().map_err(|_| format!("`{n}` is not a number"))?;
            let r = r.trim().strip_prefix("of ").unwrap_or(r.trim());
            let mut s = set_of(env, r)?;
            s.sort_by(|a, b| b.stats().map_or(f64::MIN, |t| t.avg).total_cmp(&a.stats().map_or(f64::MIN, |t| t.avg)));
            s.truncate(n);
            Ok(Val::Set(s))
        }
        "filter" => {
            let (r, cond) = rest.split_once(" where ").ok_or("write: filter A where label=value")?;
            let (neg, (k, v)) = match cond.split_once("!=") {
                Some(kv) => (true, kv),
                None => (false, cond.split_once('=').ok_or("the condition is label=value or label!=value")?),
            };
            let (k, v) = (k.trim(), unquote(v));
            Ok(Val::Set(set_of(env, r)?.into_iter().filter(|s| (s.labels.get(k).map(String::as_str) == Some(v.as_str())) != neg).collect()))
        }
        "group" => {
            let (r, tail) = rest.split_once(" by ").ok_or("write: group A by label sum")?;
            let mut it = tail.split_whitespace();
            let label = it.next().ok_or("group needs a label")?;
            Ok(Val::Set(group(set_of(env, r)?, label, it.next().unwrap_or("sum"))?))
        }
        "rename" => {
            let (r, fmt) = rest.split_once(" as ").ok_or("write: rename A as \"{{label}}\"")?;
            let f = unquote(fmt);
            Ok(Val::Set(set_of(env, r)?.into_iter().map(|mut s| {
                s.name = series_label(&f, &s.name, &s.labels);
                s
            }).collect()))
        }
        _ => eval_math(rhs, env, o),
    }
}

/// Lines that set the options instead of making a result: `align previous 30s` (nearest, previous or interpolate, and the
/// tolerance as a duration). Returns the options and the other lines.
pub fn split_directives(lines: &[String], base: Opts) -> (Opts, Vec<String>) {
    let mut o = base;
    let mut rest = vec![];
    for l in lines {
        let t = l.trim();
        if let Some(r) = t.strip_prefix("align ") {
            let mut it = r.split_whitespace();
            if let Some(a) = it.next().and_then(Align::parse) {
                o.align = a;
            }
            if let Some(ms) = it.next().and_then(super::model::parse_duration_ms) {
                o.tolerance_ms = ms;
            }
        } else {
            rest.push(l.clone());
        }
    }
    (o, rest)
}

/// Run the lines over the queries' frames (by letter). Series of the results are returned in line order; a line that fails is
/// reported and the others still run.
pub fn run(lines: &[String], frames: &BTreeMap<char, Frame>, o: &Opts) -> Output {
    let mut env: BTreeMap<String, Val> = BTreeMap::new();
    for (k, f) in frames {
        env.insert(k.to_string(), if f.series.is_empty() && !f.columns.is_empty() { Val::Table(f.clone()) } else { Val::Set(f.series.clone()) });
    }
    let mut out = Output::default();
    for line in lines {
        let l = line.split('#').next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        let Some((name, rhs)) = l.split_once('=') else {
            out.errors.push(format!("`{}`: write NAME = expression", crate::dash::frame::clip(l, 40)));
            continue;
        };
        let name = name.trim();
        if !name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            out.errors.push(format!("`{}` is not a name for a result", crate::dash::frame::clip(name, 30)));
            continue;
        }
        match eval_line(rhs, &env, o) {
            Ok(Val::Set(mut s)) => {
                let verb = rhs.split_whitespace().next().unwrap_or("");
                if !matches!(verb, "merge" | "join" | "top" | "filter" | "group" | "rename") {
                    let single = s.len() == 1;
                    for x in &mut s {
                        x.name = if single { name.to_string() } else { format!("{name} {}", x.name) };
                    }
                }
                out.series.extend(s.iter().cloned());
                env.insert(name.to_string(), Val::Set(s));
            }
            Ok(Val::Table(t)) => {
                out.table = Some(t.clone());
                env.insert(name.to_string(), Val::Table(t));
            }
            Ok(Val::Num(n)) => {
                env.insert(name.to_string(), Val::Num(n));
            }
            Err(e) => out.errors.push(format!("{name}: {e}")),
        }
    }
    out
}

/// Check the lines without data (the editor shows the first problem as you type).
pub fn check(lines: &[String]) -> Vec<String> {
    let mut frames = BTreeMap::new();
    for l in 'A'..='H' {
        frames.insert(l, Frame { series: vec![Series { name: l.to_string(), points: vec![(0, 1.0), (60_000, 2.0)], ..Default::default() }], ..Default::default() });
    }
    run(lines, &frames, &Opts::default()).errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ser(name: &str, host: &str, pts: &[(i64, f64)]) -> Series {
        Series { name: name.into(), labels: [("host".to_string(), host.to_string())].into(), points: pts.to_vec(), axis: 0 }
    }
    fn frames() -> BTreeMap<char, Frame> {
        let mut m = BTreeMap::new();
        m.insert('A', Frame { series: vec![ser("a1", "web1", &[(0, 10.0), (60_000, 20.0), (120_000, 30.0)]), ser("a2", "web2", &[(0, 1.0), (60_000, 2.0), (120_000, 3.0)])], ..Default::default() });
        // B has its own timestamps: 5 s late, and a gap
        m.insert('B', Frame { series: vec![ser("b1", "web1", &[(5_000, 100.0), (65_000, 200.0), (125_000, 300.0)]), ser("b2", "web2", &[(5_000, 10.0), (125_000, 30.0)])], ..Default::default() });
        m
    }
    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn math_between_queries_aligns_by_labels_and_time() {
        let o = run(&lines(&["C = A / B * 100"]), &frames(), &Opts::default());
        assert!(o.errors.is_empty(), "{:?}", o.errors);
        assert_eq!(o.series.len(), 2);
        let web1 = o.series.iter().find(|s| s.labels["host"] == "web1").unwrap();
        assert_eq!(web1.points, vec![(0, 10.0), (60_000, 10.0), (120_000, 10.0)]);
        assert!(web1.name.starts_with("C "), "{}", web1.name);
        // web2 has a gap in B: the point at 60 s is matched to the nearest (5 s or 125 s late, whichever is closer and inside the tolerance)
        let web2 = o.series.iter().find(|s| s.labels["host"] == "web2").unwrap();
        assert_eq!(web2.points[0], (0, 10.0));
    }

    #[test]
    fn the_three_alignments_differ_where_the_partner_has_gaps() {
        let a = vec![(0, 1.0), (30_000, 1.0), (60_000, 1.0)];
        let b = vec![(0, 0.0), (60_000, 60.0)];
        assert_eq!(value_at(&b, 30_000, Align::Previous, 40_000), Some(0.0));
        assert_eq!(value_at(&b, 30_000, Align::Interpolate, 40_000), Some(30.0));
        assert!(matches!(value_at(&b, 20_000, Align::Nearest, 25_000), Some(v) if v == 0.0));
        assert_eq!(value_at(&b, 30_000, Align::Nearest, 10_000), None, "outside the tolerance");
        assert_eq!(value_at(&b, 200_000, Align::Interpolate, 10_000), None);
        let mut f = BTreeMap::new();
        f.insert('A', Frame { series: vec![Series { name: "a".into(), points: a, ..Default::default() }], ..Default::default() });
        f.insert('B', Frame { series: vec![Series { name: "b".into(), points: b, ..Default::default() }], ..Default::default() });
        let interp = run(&lines(&["C = A + B"]), &f, &Opts { align: Align::Interpolate, tolerance_ms: 40_000 });
        assert_eq!(interp.series[0].points, vec![(0, 1.0), (30_000, 31.0), (60_000, 61.0)]);
        let prev = run(&lines(&["C = A + B"]), &f, &Opts { align: Align::Previous, tolerance_ms: 40_000 });
        assert_eq!(prev.series[0].points[1], (30_000, 1.0));
    }

    #[test]
    fn numbers_rate_abs_and_division_by_zero() {
        let o = run(&lines(&["R = rate(A) * 60", "Z = A / 0", "N = abs(A - 100)"]), &frames(), &Opts::default());
        assert_eq!(o.series.iter().find(|s| s.name.starts_with("R ") && s.labels["host"] == "web1").unwrap().points, vec![(60_000, 10.0), (120_000, 10.0)]);
        assert!(!o.series.iter().any(|s| s.name.starts_with("Z")) || o.series.iter().filter(|s| s.name.starts_with("Z")).all(|s| s.points.is_empty()), "no infinities");
        assert!(o.series.iter().any(|s| s.name.starts_with("N ") && s.points[0].1 == 90.0));
    }

    #[test]
    fn top_filter_group_merge_rename() {
        let o = run(&lines(&["T = top 1 A", "F = filter A where host=web2", "X = filter A where host!=web2", "G = group A by host sum", "M = merge A, B", "R = rename A as \"{{host}} cpu\""]), &frames(), &Opts::default());
        assert!(o.errors.is_empty(), "{:?}", o.errors);
        let names: Vec<&str> = o.series.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"a1") && names.contains(&"web2 cpu") && names.contains(&"web1 cpu"), "{names:?}");
        assert_eq!(o.series.iter().filter(|s| s.name == "a2").count(), 2, "filter F and merge M keep a2");
        assert_eq!(o.series.iter().filter(|s| s.name == "web1").count(), 1, "group by host names the series after the value");
        let top = run(&lines(&["T = top 1 A"]), &frames(), &Opts::default());
        assert_eq!(top.series.len(), 1);
        assert_eq!(top.series[0].name, "a1");
    }

    #[test]
    fn a_table_join_on_a_column() {
        use serde_json::json;
        let ch = Frame { columns: vec![Column { name: "key".into(), ty: ColType::Str }, Column { name: "ms".into(), ty: ColType::Float }], rows: vec![vec![json!("a"), json!(1.5)], vec![json!("b"), json!(2.5)], vec![json!("c"), json!(9)]], ..Default::default() };
        let es = Frame { columns: vec![Column { name: "key".into(), ty: ColType::Str }, Column { name: "doc_count".into(), ty: ColType::Int }, Column { name: "ms".into(), ty: ColType::Float }], rows: vec![vec![json!("b"), json!(20), json!(0)], vec![json!("a"), json!(10), json!(0)], vec![json!("a"), json!(11), json!(0)]], ..Default::default() };
        let mut f = BTreeMap::new();
        f.insert('A', ch);
        f.insert('B', es);
        let o = run(&lines(&["T = join A, B on key"]), &f, &Opts::default());
        let t = o.table.unwrap();
        assert_eq!(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["key", "ms", "doc_count", "ms_2"]);
        assert_eq!(t.rows.len(), 3, "a twice, b once, c dropped");
        assert_eq!(t.cell(2, 2), "20");
        let bad = run(&lines(&["T = join A, B on nope"]), &f, &Opts::default());
        assert!(bad.errors[0].contains("both tables need a column `nope`"), "{:?}", bad.errors);
    }

    #[test]
    fn mistakes_are_per_line_and_explained() {
        let o = run(&lines(&["C = A / ", "D = Q + 1", "E = A *", "oops", "F = A + B", "1x = A", "G = join A"]), &frames(), &Opts::default());
        assert_eq!(o.errors.len(), 6, "{:?}", o.errors);
        assert!(o.series.iter().any(|s| s.name.starts_with("F")), "a good line still runs");
        assert!(o.errors.iter().any(|e| e.contains("`Q` is not a query")));
        assert!(check(&lines(&["C = A / B * 100"])).is_empty());
        assert!(!check(&lines(&["C = (A"])).is_empty());
        assert!(run(&lines(&["C = A / B"]), &BTreeMap::new(), &Opts::default()).errors[0].contains("not a query"));
    }

    #[test]
    fn unrelated_labels_are_reported_not_guessed() {
        let mut f = frames();
        f.get_mut(&'B').unwrap().series = vec![ser("b1", "db1", &[(0, 1.0)]), ser("b2", "db2", &[(0, 1.0)]), ser("b3", "db3", &[(0, 1.0)])];
        let o = run(&lines(&["C = A / B"]), &f, &Opts::default());
        assert!(o.errors[0].contains("no series of the two sides belong together"), "{:?}", o.errors);
    }
}
