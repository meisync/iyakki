//! What a query returns: a [`Frame`] holds a table (typed columns and rows), time series (a name, labels and timestamped
//! points), or both. Every connector produces one, every chart and table reads one. All of it is `serde`, so the seam can
//! sit behind a remote backend later.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ColType {
    Int,
    Float,
    #[default]
    Str,
    Bool,
    /// Text or a number that is a point in time (shown as a date, usable as the x axis).
    Time,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub ty: ColType,
}

/// One line of a chart: timestamps in milliseconds since the epoch, finite values only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Series {
    pub name: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub points: Vec<(i64, f64)>,
    /// 0 = left axis, 1 = right axis.
    #[serde(default)]
    pub axis: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Meta {
    /// A limit (rows, series, bytes) cut the answer short.
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub took_ms: u64,
    /// Sentences for the person ("1 of 40 series shown").
    #[serde(default)]
    pub notices: Vec<String>,
    /// The text actually sent to the server (after variables were filled in); never holds a credential.
    #[serde(default)]
    pub executed: String,
    /// Unit name of the right axis (see `Unit::parse`).
    #[serde(default)]
    pub right_unit: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Frame {
    #[serde(default)]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub rows: Vec<Vec<Value>>,
    #[serde(default)]
    pub series: Vec<Series>,
    #[serde(default)]
    pub meta: Meta,
}

/// Summary numbers of one series.
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
    pub last: f64,
    pub first: f64,
    pub min_at: i64,
    pub max_at: i64,
    pub std: f64,
}

impl Series {
    pub fn stats(&self) -> Option<Stats> {
        let pts: Vec<&(i64, f64)> = self.points.iter().filter(|p| p.1.is_finite()).collect();
        if pts.is_empty() {
            return None;
        }
        let n = pts.len() as f64;
        let avg = pts.iter().map(|p| p.1).sum::<f64>() / n;
        let var = pts.iter().map(|p| (p.1 - avg).powi(2)).sum::<f64>() / n;
        let (mut mn, mut mx) = (pts[0], pts[0]);
        for p in &pts {
            if p.1 < mn.1 {
                mn = p;
            }
            if p.1 > mx.1 {
                mx = p;
            }
        }
        Some(Stats { count: pts.len(), min: mn.1, max: mx.1, avg, last: pts[pts.len() - 1].1, first: pts[0].1, min_at: mn.0, max_at: mx.0, std: var.sqrt() })
    }
}

impl Frame {
    pub fn empty() -> Frame {
        Frame::default()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.series.iter().all(|s| s.points.is_empty())
    }

    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    /// The cell as text (numbers trimmed, null empty).
    pub fn cell(&self, row: usize, col: usize) -> String {
        self.rows.get(row).and_then(|r| r.get(col)).map(cell_text).unwrap_or_default()
    }

    pub fn number(&self, row: usize, col: usize) -> Option<f64> {
        self.rows.get(row).and_then(|r| r.get(col)).and_then(value_f64)
    }

    /// Series made from a table: one per distinct value of `group` (all rows, one series, when `None`) per value column.
    /// Rows whose time or value does not parse are skipped. Points come out sorted by time.
    pub fn series_from_table(&self, time: &str, values: &[&str], group: &[&str]) -> Vec<Series> {
        let Some(ti) = self.column_index(time) else { return vec![] };
        let gi: Vec<usize> = group.iter().filter_map(|g| self.column_index(g)).collect();
        let vi: Vec<(usize, &str)> = values.iter().filter_map(|v| self.column_index(v).map(|i| (i, *v))).collect();
        let mut out: BTreeMap<(String, String), Series> = BTreeMap::new();
        for r in &self.rows {
            let Some(t) = r.get(ti).and_then(value_time_ms) else { continue };
            let labels: BTreeMap<String, String> = gi.iter().map(|&i| (self.columns[i].name.clone(), r.get(i).map(cell_text).unwrap_or_default())).collect();
            let gname = labels.values().cloned().collect::<Vec<_>>().join(", ");
            for &(i, vname) in &vi {
                let Some(v) = r.get(i).and_then(value_f64).filter(|v| v.is_finite()) else { continue };
                let name = match (gname.is_empty(), vi.len() > 1) {
                    (true, _) => vname.to_string(),
                    (false, true) => format!("{gname} {vname}"),
                    (false, false) => gname.clone(),
                };
                out.entry((gname.clone(), vname.to_string())).or_insert_with(|| Series { name, labels: labels.clone(), points: vec![], axis: 0 }).points.push((t, v));
            }
        }
        let mut v: Vec<Series> = out.into_values().collect();
        for s in &mut v {
            s.points.sort_by_key(|p| p.0);
        }
        v
    }

    /// If the frame is only a table with a time column and numeric columns, fill `series` from it (first time column,
    /// numeric columns as values, and the text columns as the grouping). Does nothing when series exist already.
    pub fn derive_series(&mut self) {
        if !self.series.is_empty() {
            return;
        }
        let Some(t) = self.columns.iter().find(|c| c.ty == ColType::Time) else { return };
        let time = t.name.clone();
        let vals: Vec<String> = self.columns.iter().filter(|c| matches!(c.ty, ColType::Int | ColType::Float)).map(|c| c.name.clone()).collect();
        let grp: Vec<String> = self.columns.iter().filter(|c| c.ty == ColType::Str).map(|c| c.name.clone()).collect();
        if vals.is_empty() {
            return;
        }
        let (v, g): (Vec<&str>, Vec<&str>) = (vals.iter().map(String::as_str).collect(), grp.iter().map(String::as_str).collect());
        self.series = self.series_from_table(&time, &v, &g);
    }

    /// Time series as a table: `time` then one column per series (blank where a series has no point at that time).
    pub fn series_table(&self) -> Frame {
        let mut times: Vec<i64> = self.series.iter().flat_map(|s| s.points.iter().map(|p| p.0)).collect();
        times.sort_unstable();
        times.dedup();
        let mut columns = vec![Column { name: "time".into(), ty: ColType::Time }];
        columns.extend(self.series.iter().map(|s| Column { name: s.name.clone(), ty: ColType::Float }));
        let idx: Vec<BTreeMap<i64, f64>> = self.series.iter().map(|s| s.points.iter().cloned().collect()).collect();
        let rows = times.iter().map(|t| std::iter::once(Value::from(*t)).chain(idx.iter().map(|m| m.get(t).map_or(Value::Null, |v| num(*v)))).collect()).collect();
        Frame { columns, rows, series: vec![], meta: self.meta.clone() }
    }

    /// Cut to at most `rows` rows and `points` points per series, setting `meta.truncated` when something was dropped.
    pub fn cap(&mut self, rows: usize, series: usize, points: usize) {
        if self.rows.len() > rows {
            self.rows.truncate(rows);
            self.meta.truncated = true;
            self.meta.notices.push(format!("only the first {rows} rows are kept"));
        }
        if self.series.len() > series {
            let n = self.series.len();
            self.series.truncate(series);
            self.meta.truncated = true;
            self.meta.notices.push(format!("{series} of {n} series are shown"));
        }
        for s in &mut self.series {
            if s.points.len() > points {
                let keep = s.points.len() - points;
                s.points.drain(..keep);
                self.meta.truncated = true;
            }
        }
    }

    /// Markdown for the assistant: a table (at most `max_rows` rows) or, for series, one line of summary numbers each.
    pub fn markdown(&self, max_rows: usize, max_chars: usize) -> String {
        let mut out = String::new();
        if !self.series.is_empty() {
            out.push_str("| series | points | last | min | max | avg |\n|---|---|---|---|---|---|\n");
            for s in self.series.iter().take(max_rows) {
                match s.stats() {
                    Some(t) => out.push_str(&format!("| {} | {} | {} | {} | {} | {} |\n", md_cell(&s.name), t.count, short(t.last), short(t.min), short(t.max), short(t.avg))),
                    None => out.push_str(&format!("| {} | 0 | | | | |\n", md_cell(&s.name))),
                }
            }
            if self.series.len() > max_rows {
                out.push_str(&format!("({} more series not shown)\n", self.series.len() - max_rows));
            }
        }
        if !self.columns.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("| {} |\n|{}\n", self.columns.iter().map(|c| md_cell(&c.name)).collect::<Vec<_>>().join(" | "), "---|".repeat(self.columns.len())));
            for r in 0..self.rows.len().min(max_rows) {
                let cells: Vec<String> = (0..self.columns.len()).map(|c| md_cell(&clip(&self.cell(r, c), 60))).collect();
                out.push_str(&format!("| {} |\n", cells.join(" | ")));
                if out.chars().count() > max_chars {
                    out.push_str("(cut: the answer is long)\n");
                    return out;
                }
            }
            if self.rows.len() > max_rows {
                out.push_str(&format!("({} more rows not shown)\n", self.rows.len() - max_rows));
            }
        }
        if out.is_empty() {
            out.push_str("(no data)\n");
        }
        if out.chars().count() > max_chars {
            out = out.chars().take(max_chars).collect::<String>() + "\n(cut: the answer is long)\n";
        }
        out
    }

    /// The table as CSV (RFC 4180 quoting); series are turned into a table first.
    pub fn csv(&self) -> String {
        let t = if self.columns.is_empty() && !self.series.is_empty() { self.series_table() } else { self.clone() };
        let q = |s: &str| if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
        let mut out = t.columns.iter().map(|c| q(&c.name)).collect::<Vec<_>>().join(",") + "\n";
        for r in 0..t.rows.len() {
            out.push_str(&(0..t.columns.len()).map(|c| q(&t.cell(r, c))).collect::<Vec<_>>().join(","));
            out.push('\n');
        }
        out
    }

    /// A few lines for the assistant's context and for tooltips: what the frame holds, with numbers.
    pub fn summary(&self) -> String {
        let mut p = vec![];
        if !self.series.is_empty() {
            p.push(format!("{} series", self.series.len()));
            for s in self.series.iter().take(3) {
                if let Some(t) = s.stats() {
                    p.push(format!("{}: last {}, min {}, max {}, avg {}", clip(&s.name, 40), short(t.last), short(t.min), short(t.max), short(t.avg)));
                }
            }
        }
        if !self.columns.is_empty() {
            p.push(format!("{} rows x {} columns ({})", self.rows.len(), self.columns.len(), self.columns.iter().take(8).map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")));
        }
        if self.meta.truncated {
            p.push("cut by a limit".into());
        }
        if p.is_empty() { "no data".into() } else { p.join("; ") }
    }
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n.saturating_sub(1)).collect::<String>() + "…" }
}

/// A number with at most four significant digits and no trailing zeros.
pub fn short(v: f64) -> String {
    if !v.is_finite() {
        return "n/a".into();
    }
    let a = v.abs();
    if a != 0.0 && !(0.001..1e12).contains(&a) {
        return format!("{v:.3e}");
    }
    let s = if a >= 1000.0 { format!("{v:.0}") } else if a >= 100.0 { format!("{v:.1}") } else if a >= 1.0 { format!("{v:.2}") } else { format!("{v:.4}") };
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

pub fn num(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9e15 {
        Value::from(v as i64)
    } else {
        serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
    }
}

pub fn cell_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_f64().filter(|f| f.fract() != 0.0).map_or_else(|| n.to_string(), short),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

pub fn value_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => parse_f64(s),
        Value::Bool(b) => Some(*b as u8 as f64),
        _ => None,
    }
}

/// A float from text, accepting what Prometheus and ClickHouse print (`NaN` and the infinities come back as `None`).
pub fn parse_f64(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|f| f.is_finite())
}

pub fn value_time_ms(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => {
            let f = n.as_f64()?;
            Some(if f.abs() < 1e11 { (f * 1000.0) as i64 } else { f as i64 })
        }
        Value::String(s) => parse_time_ms(s),
        _ => None,
    }
}

/// Epoch milliseconds from `2026-10-15T12:00:00Z`, `2026-10-15 12:00:00.250`, `2026-10-15` (UTC), a number of seconds, or
/// a number of milliseconds.
pub fn parse_time_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(f) = s.parse::<f64>() {
        return f.is_finite().then(|| if f.abs() < 1e11 { (f * 1000.0) as i64 } else { f as i64 });
    }
    let b = s.as_bytes();
    let d = |i: usize, n: usize| -> Option<i64> { s.get(i..i + n)?.parse::<i64>().ok() };
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, mo, da) = (d(0, 4)?, d(5, 2)?, d(8, 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&da) {
        return None;
    }
    let (mut h, mut mi, mut se, mut ms) = (0, 0, 0, 0);
    let mut rest = &s[10..];
    if rest.starts_with(['T', ' ']) && rest.len() >= 6 {
        h = d(11, 2)?;
        mi = d(14, 2)?;
        rest = &s[16..];
        if rest.starts_with(':') {
            se = d(17, 2)?;
            rest = &s[19..];
            if let Some(frac) = rest.strip_prefix('.') {
                let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
                let three = format!("{digits:0<3}");
                ms = three[..3].parse().ok()?;
                rest = &frac[digits.len()..];
            }
        }
    }
    let mut offset_min = 0i64;
    let rest = rest.trim();
    if rest.starts_with(['+', '-']) && rest.len() >= 3 {
        let sign = if rest.starts_with('-') { -1 } else { 1 };
        let hh: i64 = rest.get(1..3)?.parse().ok()?;
        let mm: i64 = rest.get(3..).map(|r| r.trim_start_matches(':')).and_then(|r| r.get(..2)).and_then(|r| r.parse().ok()).unwrap_or(0);
        offset_min = sign * (hh * 60 + mm);
    } else if !(rest.is_empty() || rest == "Z" || rest.eq_ignore_ascii_case("utc")) {
        return None;
    }
    Some(days_from_civil(y, mo, da) * 86_400_000 + h * 3_600_000 + mi * 60_000 + se * 1000 + ms - offset_min * 60_000)
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `2026-10-15 12:00:05` (UTC) from epoch milliseconds.
pub fn format_time(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", t / 3600, t % 3600 / 60, t % 60)
}

/// Guess a column's type from its values (nulls ignored): ints, floats, bools, times (only when `name` looks like one), else text.
pub fn infer_type(name: &str, values: impl Iterator<Item = impl std::borrow::Borrow<Value>>) -> ColType {
    let (mut int, mut float, mut boo, mut time, mut n) = (true, true, true, true, 0);
    let timey = {
        let l = name.to_ascii_lowercase();
        l.contains("time") || l.contains("date") || l.contains("timestamp") || l == "ts" || l == "t" || l == "day" || l == "dt" || l == "when" || l == "hour" || l == "minute" || l.starts_with('@') || l.ends_with("_at")
    };
    for v in values {
        match v.borrow() {
            Value::Null => continue,
            Value::Number(x) => {
                n += 1;
                boo = false;
                time = time && timey && x.as_f64().is_some_and(|f| f.abs() > 1e8);
                int &= x.is_i64() || x.is_u64();
            }
            Value::Bool(_) => {
                n += 1;
                int = false;
                float = false;
                time = false;
            }
            Value::String(s) => {
                n += 1;
                int = false;
                float = false;
                boo = false;
                time = time && timey && parse_time_ms(s).is_some() && s.len() >= 8;
            }
            _ => {
                n += 1;
                int = false;
                float = false;
                boo = false;
                time = false;
            }
        }
    }
    if n == 0 {
        ColType::Str
    } else if time {
        ColType::Time
    } else if boo {
        ColType::Bool
    } else if int {
        ColType::Int
    } else if float {
        ColType::Float
    } else {
        ColType::Str
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn times_parse_and_format_round_trip() {
        assert_eq!(parse_time_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time_ms("2026-10-15 12:00:05.250"), Some(1_792_065_605_250));
        assert_eq!(format_time(1_792_065_605_250), "2026-10-15 12:00:05");
        assert_eq!(parse_time_ms("2026-10-15T14:00:00+02:00"), parse_time_ms("2026-10-15T12:00:00Z"));
        assert_eq!(parse_time_ms("2026-10-15"), Some(1_792_022_400_000));
        assert_eq!(parse_time_ms("1700000000"), Some(1_700_000_000_000));
        assert_eq!(parse_time_ms("1700000000123"), Some(1_700_000_000_123));
        for bad in ["", "hello", "2026-13-01", "2026-10-15 xx", "2026/10/15"] {
            assert_eq!(parse_time_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn civil_days_are_inverse_for_many_dates() {
        for z in (-800_000..800_000).step_by(997) {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    #[test]
    fn series_come_from_a_table_grouped_and_sorted() {
        let f = Frame {
            columns: vec![Column { name: "t".into(), ty: ColType::Time }, Column { name: "host".into(), ty: ColType::Str }, Column { name: "cpu".into(), ty: ColType::Float }],
            rows: vec![vec![json!("2026-10-15 12:01:00"), json!("a"), json!(2.0)], vec![json!("2026-10-15 12:00:00"), json!("a"), json!(1.0)], vec![json!("2026-10-15 12:00:00"), json!("b"), json!(5)], vec![json!("junk"), json!("b"), json!(5)], vec![json!("2026-10-15 12:00:00"), json!("b"), json!(null)]],
            ..Default::default()
        };
        let s = f.series_from_table("t", &["cpu"], &["host"]);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "a");
        assert_eq!(s[0].points.iter().map(|p| p.1).collect::<Vec<_>>(), vec![1.0, 2.0]);
        assert_eq!(s[1].points.len(), 1);
        let mut g = f.clone();
        g.derive_series();
        assert_eq!(g.series.len(), 2);
    }

    #[test]
    fn stats_markdown_csv_and_caps() {
        let s = Series { name: "x|y".into(), points: vec![(1000, 1.0), (2000, 9.0), (3000, 5.0)], ..Default::default() };
        let t = s.stats().unwrap();
        assert_eq!((t.min, t.max, t.avg, t.last, t.max_at), (1.0, 9.0, 5.0, 5.0, 2000));
        let mut f = Frame { series: vec![s], ..Default::default() };
        let md = f.markdown(10, 1000);
        assert!(md.contains("x\\|y") && md.contains("| 3 |"), "{md}");
        let csv = f.csv();
        assert!(csv.starts_with("time,x|y\n1000,1\n"), "{csv}");
        f.cap(1, 1, 2);
        assert_eq!(f.series[0].points.len(), 2);
        assert!(f.meta.truncated);
        assert_eq!(Frame::empty().markdown(5, 100), "(no data)\n");
        let q = Frame { columns: vec![Column { name: "a,b".into(), ty: ColType::Str }], rows: vec![vec![json!("he said \"hi\"")]], ..Default::default() };
        assert_eq!(q.csv(), "\"a,b\"\n\"he said \"\"hi\"\"\"\n");
    }

    #[test]
    fn numbers_print_short_and_odd_values_do_not_panic() {
        assert_eq!(short(1234.5678), "1235");
        assert_eq!(short(0.5), "0.5");
        assert_eq!(short(f64::NAN), "n/a");
        assert_eq!(short(1.5e-7), "1.500e-7");
        assert_eq!(parse_f64("NaN"), None);
        assert_eq!(parse_f64("+Inf"), None);
        assert_eq!(cell_text(&json!({"a": [1]})), "{\"a\":[1]}");
        assert_eq!(infer_type("created_at", [json!("2026-10-15 12:00:00")].iter()), ColType::Time);
        assert_eq!(infer_type("name", [json!("2026-10-15 12:00:00")].iter()), ColType::Str);
        assert_eq!(infer_type("n", [json!(1), json!(2.5)].iter()), ColType::Float);
        assert_eq!(infer_type("n", [json!(1), json!(null)].iter()), ColType::Int);
    }
}
