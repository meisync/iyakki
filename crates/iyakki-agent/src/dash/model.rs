//! The dashboard document: panels on a 24-column grid, each with a source, a query and a way to draw the answer; template
//! variables and a time range for the whole dashboard. Saved as JSON; `version` lets later builds read older files.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::frame::{format_time, short};

pub const GRID_COLS: i32 = 24;
pub const MAX_PANELS: usize = 60;
pub const MAX_TITLE: usize = 80;
pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Visual {
    #[default]
    Line,
    Area,
    Bar,
    StackedBar,
    Pie,
    Donut,
    Gauge,
    Stat,
    Heatmap,
    Table,
    Text,
}

impl Visual {
    pub const ALL: [Visual; 11] = [Visual::Line, Visual::Area, Visual::Bar, Visual::StackedBar, Visual::Pie, Visual::Donut, Visual::Gauge, Visual::Stat, Visual::Heatmap, Visual::Table, Visual::Text];
    pub fn label(self) -> &'static str {
        match self {
            Visual::Line => "Line",
            Visual::Area => "Area",
            Visual::Bar => "Bar",
            Visual::StackedBar => "Stacked bar",
            Visual::Pie => "Pie",
            Visual::Donut => "Donut",
            Visual::Gauge => "Gauge",
            Visual::Stat => "Stat",
            Visual::Heatmap => "Heatmap",
            Visual::Table => "Table",
            Visual::Text => "Text",
        }
    }
    pub fn parse(s: &str) -> Option<Visual> {
        let k = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Visual::ALL.into_iter().find(|v| serde_json::to_value(v).ok().and_then(|x| x.as_str().map(String::from)).as_deref() == Some(k.as_str()) || v.label().to_ascii_lowercase().replace(' ', "_") == k)
            .or(match k.as_str() {
                "timeseries" | "time_series" | "graph" => Some(Visual::Line),
                "number" | "singlestat" | "big_number" => Some(Visual::Stat),
                "barchart" | "bar_chart" => Some(Visual::Bar),
                "markdown" | "note" => Some(Visual::Text),
                _ => None,
            })
    }
    /// Draws the points of series over time.
    pub fn is_timeseries(self) -> bool {
        matches!(self, Visual::Line | Visual::Area)
    }
    pub fn needs_query(self) -> bool {
        self != Visual::Text
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    #[default]
    None,
    Number,
    Percent,
    /// 0..1 shown as a percent.
    Percent01,
    Bytes,
    BytesPerSec,
    Seconds,
    Millis,
    Date,
}

impl Unit {
    pub const ALL: [Unit; 9] = [Unit::None, Unit::Number, Unit::Percent, Unit::Percent01, Unit::Bytes, Unit::BytesPerSec, Unit::Seconds, Unit::Millis, Unit::Date];
    pub fn label(self) -> &'static str {
        match self {
            Unit::None => "none",
            Unit::Number => "number",
            Unit::Percent => "percent (0-100)",
            Unit::Percent01 => "percent (0-1)",
            Unit::Bytes => "bytes",
            Unit::BytesPerSec => "bytes/s",
            Unit::Seconds => "seconds",
            Unit::Millis => "milliseconds",
            Unit::Date => "date",
        }
    }
    pub fn parse(s: &str) -> Option<Unit> {
        let k = s.trim().to_ascii_lowercase();
        match k.as_str() {
            "" | "none" => Some(Unit::None),
            "number" | "short" | "count" => Some(Unit::Number),
            "percent" | "%" => Some(Unit::Percent),
            "percent01" | "percentunit" | "ratio" => Some(Unit::Percent01),
            "bytes" | "b" => Some(Unit::Bytes),
            "bytes/s" | "bytes_per_sec" | "bps" => Some(Unit::BytesPerSec),
            "seconds" | "s" => Some(Unit::Seconds),
            "millis" | "ms" | "milliseconds" => Some(Unit::Millis),
            "date" | "time" => Some(Unit::Date),
            _ => None,
        }
    }
}

/// Format a number for display in a unit (`decimals` None picks a sensible count).
pub fn format_value(v: f64, unit: Unit, decimals: Option<u8>) -> String {
    if !v.is_finite() {
        return "n/a".into();
    }
    let d = |x: f64, def: usize| -> String {
        let n = decimals.map_or(def, |d| d as usize);
        let s = format!("{x:.n$}");
        if decimals.is_none() && s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
    };
    match unit {
        Unit::None => short(v),
        Unit::Number => {
            let a = v.abs();
            for (div, suf) in [(1e12, "T"), (1e9, "G"), (1e6, "M"), (1e3, "K")] {
                if a >= div {
                    return format!("{}{suf}", d(v / div, 2));
                }
            }
            if a >= 100.0 || v.fract() == 0.0 { d(v, 0) } else { d(v, 2) }
        }
        Unit::Percent => format!("{}%", d(v, 1)),
        Unit::Percent01 => format!("{}%", d(v * 100.0, 1)),
        Unit::Bytes | Unit::BytesPerSec => {
            let suffix = if unit == Unit::BytesPerSec { "/s" } else { "" };
            let a = v.abs();
            let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
            let mut i = 0;
            let mut x = v;
            while x.abs() >= 1024.0 && i < units.len() - 1 {
                x /= 1024.0;
                i += 1;
            }
            let _ = a;
            format!("{} {}{suffix}", d(x, if i == 0 { 0 } else { 1 }), units[i])
        }
        Unit::Seconds => duration(v),
        Unit::Millis => {
            if v.abs() < 1000.0 { format!("{} ms", d(v, 0)) } else { duration(v / 1000.0) }
        }
        Unit::Date => format_time(if v.abs() < 1e11 { (v * 1000.0) as i64 } else { v as i64 }),
    }
}

fn duration(s: f64) -> String {
    let a = s.abs();
    if a < 1e-3 {
        return format!("{} us", short(s * 1e6));
    }
    if a < 1.0 {
        return format!("{} ms", short(s * 1000.0));
    }
    if a < 60.0 {
        return format!("{} s", short(s));
    }
    let t = a.round() as u64;
    let sign = if s < 0.0 { "-" } else { "" };
    let (d, h, m, sec) = (t / 86_400, t % 86_400 / 3600, t % 3600 / 60, t % 60);
    let s = if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {sec}s")
    };
    format!("{sign}{s}")
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Threshold {
    /// From this value up the colour applies.
    pub value: f64,
    pub color: [u8; 3],
}

/// The colour for `v`: the colour of the highest threshold at or below it, or `base` when below all.
pub fn threshold_color(th: &[Threshold], v: f64, base: [u8; 3]) -> [u8; 3] {
    let mut best: Option<&Threshold> = None;
    for t in th {
        if v >= t.value && best.is_none_or(|b| t.value >= b.value) {
            best = Some(t);
        }
    }
    best.map_or(base, |t| t.color)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Grid {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Grid {
        Grid { x, y, w, h }
    }
    pub fn overlaps(&self, o: &Grid) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
    pub fn clamp(&mut self) {
        self.w = self.w.clamp(2, GRID_COLS);
        self.h = self.h.clamp(2, 40);
        self.x = self.x.clamp(0, GRID_COLS - self.w);
        self.y = self.y.clamp(0, 10_000);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Options {
    #[serde(default)]
    pub unit: Unit,
    #[serde(default)]
    pub decimals: Option<u8>,
    #[serde(default)]
    pub thresholds: Vec<Threshold>,
    #[serde(default = "yes")]
    pub legend: bool,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// Series names from labels: `{{instance}} cpu` (empty keeps the full name).
    #[serde(default)]
    pub legend_format: String,
    /// Markdown for a text panel.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub page_size: Option<usize>,
    /// Table: sort column and direction.
    #[serde(default)]
    pub sort: Option<(String, bool)>,
    /// Table: unit per column.
    #[serde(default)]
    pub column_units: BTreeMap<String, Unit>,
    /// The unit of the right-hand axis (queries set to Axis::Right).
    #[serde(default)]
    pub right_unit: Unit,
    /// Series hidden by a click on the legend (by name).
    #[serde(default)]
    pub hidden: Vec<String>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    #[default]
    Left,
    Right,
}

/// One question to one source. A panel has query A (the `source` and `query` fields, as in older files) and optionally B, C, ...
/// (`queries`), each with its own source (Grafana "Mixed" style).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PanelQuery {
    #[serde(default)]
    pub text: String,
    /// Index / table / file when the language needs one apart from the text.
    #[serde(default)]
    pub target: Option<String>,
    /// Ask for a series every `step` (time series); None lets the dashboard pick from the range and width.
    #[serde(default)]
    pub step_s: Option<u32>,
    /// Legend text for this query's series (`{{label}}`); empty keeps the names.
    #[serde(default)]
    pub alias: String,
    #[serde(default)]
    pub unit: Option<Unit>,
    #[serde(default)]
    pub axis: Axis,
    /// Not drawn (still available to expressions).
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub timeout_s: Option<u32>,
}

/// Query B, C, ...: a source (empty = the dashboard's default) and its question.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct QueryDef {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub query: PanelQuery,
}

pub const MAX_QUERIES: usize = 8;

/// `A`, `B`, ... for the position of a query.
pub fn query_letter(i: usize) -> char {
    (b'A' + (i as u8).min(25)) as char
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub query: PanelQuery,
    #[serde(default)]
    pub visual: Visual,
    pub grid: Grid,
    #[serde(default)]
    pub options: Options,
    /// Queries B, C, ... (A is `source` + `query`).
    #[serde(default)]
    pub queries: Vec<QueryDef>,
    /// Lines of the expression language (`dash/expr.rs`): `C = A / B * 100`, `T = join A, B on key`, ...
    #[serde(default)]
    pub expressions: Vec<String>,
}

impl Panel {
    /// Every query with its letter and its source (the dashboard's default where a query names none).
    pub fn query_defs(&self, default_source: &str) -> Vec<(char, QueryDef)> {
        let pick = |s: &str| if s.is_empty() { default_source.to_string() } else { s.to_string() };
        let mut v = vec![('A', QueryDef { source: pick(&self.source), query: self.query.clone() })];
        for (i, q) in self.queries.iter().enumerate().take(MAX_QUERIES - 1) {
            v.push((query_letter(i + 1), QueryDef { source: pick(&q.source), query: q.query.clone() }));
        }
        v
    }

    /// Every source the panel uses (for access checks and refresh decisions).
    pub fn sources(&self, default_source: &str) -> Vec<String> {
        let mut v: Vec<String> = self.query_defs(default_source).into_iter().map(|d| d.1.source).filter(|s| !s.is_empty()).collect();
        v.dedup();
        v
    }

    pub fn new(id: &str, title: &str, source: &str, visual: Visual, text: &str, grid: Grid) -> Panel {
        Panel { id: id.into(), title: title.into(), source: source.into(), query: PanelQuery { text: text.into(), ..Default::default() }, visual, grid, options: Options { legend: true, ..Default::default() }, queries: vec![], expressions: vec![] }
    }
    pub fn unit(mut self, u: Unit) -> Panel {
        self.options.unit = u;
        self
    }
    pub fn target(mut self, t: &str) -> Panel {
        self.query.target = Some(t.into());
        self
    }
    pub fn thresholds(mut self, t: &[(f64, [u8; 3])]) -> Panel {
        self.options.thresholds = t.iter().map(|(value, color)| Threshold { value: *value, color: *color }).collect();
        self
    }
    pub fn legend_format(mut self, f: &str) -> Panel {
        self.options.legend_format = f.into();
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VarKind {
    /// Fixed choices.
    Custom { options: Vec<String> },
    /// The first column of a query on a source.
    Query { source: String, query: String, #[serde(default)] target: Option<String> },
    /// Step choices like 1m, 5m, 1h.
    Interval { options: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    #[serde(default)]
    pub label: String,
    pub kind: VarKind,
    #[serde(default)]
    pub current: Vec<String>,
    #[serde(default)]
    pub multi: bool,
    #[serde(default)]
    pub include_all: bool,
    /// Per source (id): how the value is written for that source, `{}` standing for the value, for example
    /// `{}.prod.example.com` for an Elasticsearch host field while Prometheus keeps the plain instance name.
    #[serde(default)]
    pub mapping: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSpec {
    pub from: String,
    pub to: String,
}

impl Default for TimeSpec {
    fn default() -> Self {
        TimeSpec { from: "now-1h".into(), to: "now".into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dashboard {
    #[serde(default = "one")]
    pub version: u32,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub time: TimeSpec,
    /// Seconds between refreshes, 0 is off.
    #[serde(default)]
    pub refresh_s: u32,
    #[serde(default)]
    pub variables: Vec<Variable>,
    #[serde(default)]
    pub panels: Vec<Panel>,
    /// The source a query uses when it names none.
    #[serde(default)]
    pub default_source: String,
}

fn one() -> u32 {
    VERSION
}

pub const REFRESH_CHOICES: [u32; 6] = [0, 5, 10, 30, 60, 300];
pub const RANGE_CHOICES: [&str; 8] = ["5m", "15m", "1h", "6h", "24h", "7d", "30d", "90d"];

impl Dashboard {
    pub fn new(id: &str, title: &str) -> Dashboard {
        Dashboard { version: VERSION, id: id.into(), title: title.into(), description: String::new(), time: TimeSpec::default(), refresh_s: 0, variables: vec![], panels: vec![], default_source: String::new() }
    }

    pub fn panel(&self, id: &str) -> Option<&Panel> {
        self.panels.iter().find(|p| p.id == id)
    }
    pub fn panel_mut(&mut self, id: &str) -> Option<&mut Panel> {
        self.panels.iter_mut().find(|p| p.id == id)
    }

    /// A panel id not used yet.
    pub fn next_id(&self) -> String {
        (1..).map(|i| format!("p{i}")).find(|id| self.panel(id).is_none()).unwrap()
    }

    /// Add a panel where it fits (below the others, or beside when there is room on the last row).
    pub fn add_panel(&mut self, mut p: Panel) -> Result<String, String> {
        if self.panels.len() >= MAX_PANELS {
            return Err(format!("a dashboard holds at most {MAX_PANELS} panels"));
        }
        if p.id.is_empty() || self.panel(&p.id).is_some() {
            p.id = self.next_id();
        }
        p.title = p.title.chars().take(MAX_TITLE).collect();
        p.grid.clamp();
        let (w, h) = (p.grid.w, p.grid.h);
        p.grid = self.free_spot(w, h);
        let id = p.id.clone();
        self.panels.push(p);
        Ok(id)
    }

    /// The first free place for a w x h panel, reading left to right and top to bottom.
    pub fn free_spot(&self, w: i32, h: i32) -> Grid {
        let bottom = self.panels.iter().map(|p| p.grid.y + p.grid.h).max().unwrap_or(0);
        for y in 0..=bottom {
            for x in 0..=(GRID_COLS - w) {
                let g = Grid::new(x, y, w, h);
                if self.panels.iter().all(|p| !p.grid.overlaps(&g)) {
                    return g;
                }
            }
        }
        Grid::new(0, bottom, w, h)
    }

    pub fn remove_panel(&mut self, id: &str) -> bool {
        let n = self.panels.len();
        self.panels.retain(|p| p.id != id);
        self.panels.len() != n
    }

    /// Place `id` at a new position / size; the panels it lands on are pushed down (keeping their order) until nothing overlaps.
    pub fn place(&mut self, id: &str, g: Grid) -> bool {
        let Some(i) = self.panels.iter().position(|p| p.id == id) else { return false };
        let mut g = g;
        g.clamp();
        self.panels[i].grid = g;
        self.settle(Some(i));
        true
    }

    /// Resolve overlaps: the pinned panel stays, the others are pushed down in reading order, then everything is pulled up
    /// when the space above is free. Idempotent; the result never overlaps and stays inside the columns.
    pub fn settle(&mut self, pinned: Option<usize>) {
        for _ in 0..24 {
            let before: Vec<Grid> = self.panels.iter().map(|p| p.grid).collect();
            self.settle_once(pinned);
            if before == self.panels.iter().map(|p| p.grid).collect::<Vec<_>>() {
                break;
            }
        }
    }

    fn settle_once(&mut self, pinned: Option<usize>) {
        for p in &mut self.panels {
            p.grid.clamp();
        }
        let mut order: Vec<usize> = (0..self.panels.len()).collect();
        order.sort_by_key(|&i| (self.panels[i].grid.y, self.panels[i].grid.x, i));
        if let Some(p) = pinned {
            order.retain(|&i| i != p);
            order.insert(0, p);
        }
        let mut placed: Vec<usize> = vec![];
        for i in order {
            let mut g = self.panels[i].grid;
            // pull up
            if pinned != Some(i) {
                while g.y > 0 && placed.iter().all(|&j| !self.panels[j].grid.overlaps(&Grid { y: g.y - 1, ..g })) {
                    g.y -= 1;
                }
            }
            // push down
            loop {
                match placed.iter().map(|&j| self.panels[j].grid).find(|o| o.overlaps(&g)) {
                    Some(o) => g.y = o.y + o.h,
                    None => break,
                }
            }
            self.panels[i].grid = g;
            placed.push(i);
        }
    }

    pub fn rows(&self) -> i32 {
        self.panels.iter().map(|p| p.grid.y + p.grid.h).max().unwrap_or(0)
    }

    /// Check an imported or hand-edited document, repairing what can be repaired. Returns notes.
    pub fn sanitize(&mut self) -> Vec<String> {
        let mut notes = vec![];
        if self.version > VERSION {
            notes.push(format!("this dashboard was made by a newer version ({}), unknown parts are ignored", self.version));
        }
        self.title = self.title.chars().take(MAX_TITLE).collect();
        if self.title.trim().is_empty() {
            self.title = "Untitled".into();
        }
        if self.panels.len() > MAX_PANELS {
            self.panels.truncate(MAX_PANELS);
            notes.push(format!("only the first {MAX_PANELS} panels are kept"));
        }
        let mut seen = std::collections::HashSet::new();
        for i in 0..self.panels.len() {
            if self.panels[i].id.is_empty() || !seen.insert(self.panels[i].id.clone()) {
                let mut n = 1;
                while seen.contains(&format!("p{n}")) || self.panels.iter().any(|p| p.id == format!("p{n}")) {
                    n += 1;
                }
                self.panels[i].id = format!("p{n}");
                seen.insert(self.panels[i].id.clone());
            }
            self.panels[i].title = self.panels[i].title.chars().take(MAX_TITLE).collect();
            self.panels[i].query.text = self.panels[i].query.text.chars().take(20_000).collect();
        }
        self.settle(None);
        if self.refresh_s != 0 && self.refresh_s < 5 {
            self.refresh_s = 5;
        }
        notes
    }
}

// ------------------------------------------------------------------------------------------------------ time

/// `now`, `now-15m`, `now-7d`, a number of epoch seconds or milliseconds, or a date/time, as epoch milliseconds.
pub fn parse_time_expr(s: &str, now_ms: i64) -> Option<i64> {
    let t = s.trim();
    if t == "now" {
        return Some(now_ms);
    }
    if let Some(rest) = t.strip_prefix("now-") {
        return parse_duration_ms(rest).map(|d| now_ms - d);
    }
    if let Some(rest) = t.strip_prefix("now+") {
        return parse_duration_ms(rest).map(|d| now_ms + d);
    }
    super::frame::parse_time_ms(t)
}

/// `90s`, `15m`, `6h`, `7d`, `2w`, `1h30m`.
pub fn parse_duration_ms(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let (mut total, mut num) = (0i64, String::new());
    for c in t.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else {
            let n: f64 = num.parse().ok()?;
            num.clear();
            let unit = match c {
                's' => 1000.0,
                'm' => 60_000.0,
                'h' => 3_600_000.0,
                'd' => 86_400_000.0,
                'w' => 604_800_000.0,
                _ => return None,
            };
            total += (n * unit) as i64;
        }
    }
    if !num.is_empty() {
        return None;
    }
    Some(total).filter(|t| *t > 0)
}

/// `1h`, `90s` ... for a duration in ms (used for `$__interval` and the range label).
pub fn duration_text(ms: i64) -> String {
    let s = (ms / 1000).max(1);
    if s % 86_400 == 0 {
        format!("{}d", s / 86_400)
    } else if s % 3600 == 0 {
        format!("{}h", s / 3600)
    } else if s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// The dashboard's range in epoch milliseconds (from < to guaranteed).
pub fn resolve_range(t: &TimeSpec, now_ms: i64) -> super::connector::TimeRange {
    let to = parse_time_expr(&t.to, now_ms).unwrap_or(now_ms);
    let from = parse_time_expr(&t.from, now_ms).unwrap_or(to - 3_600_000);
    let (from, to) = if from < to { (from, to) } else { (to - 3_600_000, to) };
    super::connector::TimeRange { from_ms: from, to_ms: to }
}

/// Legend text: `{{label}}` replaced by the series' label values; an unknown label becomes empty.
pub fn series_label(format: &str, name: &str, labels: &BTreeMap<String, String>) -> String {
    if format.trim().is_empty() {
        return name.to_string();
    }
    let mut out = String::new();
    let mut rest = format;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("}}") {
            Some(j) => {
                let key = rest[i + 2..i + 2 + j].trim();
                out.push_str(if key == "name" { name } else { labels.get(key).map_or("", String::as_str) });
                rest = &rest[i + 2 + j + 2..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let o = out.trim().to_string();
    if o.is_empty() { name.to_string() } else { o }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d_with(n: usize) -> Dashboard {
        let mut d = Dashboard::new("d", "D");
        for i in 0..n {
            d.add_panel(Panel::new("", &format!("P{i}"), "s", Visual::Line, "up", Grid::new(0, 0, 8 + (i as i32 % 3) * 4, 6))).unwrap();
        }
        d
    }

    fn no_overlap(d: &Dashboard) -> bool {
        d.panels.iter().enumerate().all(|(i, a)| a.grid.x >= 0 && a.grid.x + a.grid.w <= GRID_COLS && a.grid.y >= 0 && d.panels.iter().skip(i + 1).all(|b| !a.grid.overlaps(&b.grid)))
    }

    #[test]
    fn added_panels_never_overlap_and_fill_rows() {
        let d = d_with(12);
        assert!(no_overlap(&d));
        assert_eq!(d.panels[0].grid, Grid::new(0, 0, 8, 6));
        assert_eq!(d.panels[1].grid.x, 8, "beside the first");
        let ids: std::collections::HashSet<_> = d.panels.iter().map(|p| &p.id).collect();
        assert_eq!(ids.len(), 12);
    }

    #[test]
    fn moving_and_resizing_pushes_others_down() {
        let mut d = d_with(6);
        assert!(d.place("p1", Grid::new(3, 2, 20, 8)));
        assert!(no_overlap(&d));
        assert_eq!(d.panel("p1").unwrap().grid, Grid::new(3, 2, 20, 8));
        assert!(d.place("p2", Grid::new(-5, -5, 100, 100)));
        assert!(no_overlap(&d));
        assert_eq!(d.panel("p2").unwrap().grid.w, GRID_COLS);
        assert!(!d.place("nope", Grid::new(0, 0, 4, 4)));
    }

    #[test]
    fn layout_properties_hold_for_pseudo_random_edits() {
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut rnd = |m: i32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % m as u64) as i32
        };
        for _ in 0..40 {
            let mut d = d_with(1 + rnd(14) as usize);
            for _ in 0..30 {
                match rnd(3) {
                    0 => {
                        let _ = d.add_panel(Panel::new("", "x", "s", Visual::Stat, "q", Grid::new(rnd(30) - 3, rnd(20), rnd(30), rnd(12))));
                    }
                    1 if !d.panels.is_empty() => {
                        let id = d.panels[rnd(d.panels.len() as i32) as usize].id.clone();
                        d.place(&id, Grid::new(rnd(40) - 8, rnd(30) - 4, rnd(30) - 2, rnd(20) - 2));
                    }
                    _ if !d.panels.is_empty() => {
                        let id = d.panels[rnd(d.panels.len() as i32) as usize].id.clone();
                        d.remove_panel(&id);
                    }
                    _ => {}
                }
                assert!(no_overlap(&d), "{:?}", d.panels.iter().map(|p| p.grid).collect::<Vec<_>>());
                d.settle(None);
                assert!(no_overlap(&d));
                let before: Vec<Grid> = d.panels.iter().map(|p| p.grid).collect();
                d.settle(None);
                assert_eq!(before, d.panels.iter().map(|p| p.grid).collect::<Vec<_>>(), "settle is idempotent");
            }
        }
    }

    #[test]
    fn units_format() {
        assert_eq!(format_value(1536.0, Unit::Bytes, None), "1.5 KiB");
        assert_eq!(format_value(3.0 * 1024.0 * 1024.0 * 1024.0, Unit::Bytes, None), "3 GiB");
        assert_eq!(format_value(0.256, Unit::Percent01, None), "25.6%");
        assert_eq!(format_value(99.94, Unit::Percent, Some(0)), "100%");
        assert_eq!(format_value(0.0123, Unit::Seconds, None), "12.3 ms");
        assert_eq!(format_value(3725.0, Unit::Seconds, None), "1h 2m");
        assert_eq!(format_value(90_000.0, Unit::Millis, None), "1m 30s");
        assert_eq!(format_value(250.0, Unit::Millis, None), "250 ms");
        assert_eq!(format_value(1_250_000.0, Unit::Number, None), "1.25M");
        assert_eq!(format_value(42.0, Unit::Number, None), "42");
        assert_eq!(format_value(1_792_065_600.0, Unit::Date, None), "2026-10-15 12:00:00");
        assert_eq!(format_value(f64::NAN, Unit::Bytes, None), "n/a");
        assert_eq!(format_value(-2048.0, Unit::Bytes, None), "-2 KiB");
        assert_eq!(format_value(1e30, Unit::Bytes, None).ends_with("PiB"), true);
    }

    #[test]
    fn thresholds_pick_the_highest_reached() {
        let th = [Threshold { value: 70.0, color: [255, 200, 0] }, Threshold { value: 90.0, color: [255, 0, 0] }];
        assert_eq!(threshold_color(&th, 10.0, [0, 255, 0]), [0, 255, 0]);
        assert_eq!(threshold_color(&th, 80.0, [0, 255, 0]), [255, 200, 0]);
        assert_eq!(threshold_color(&th, 95.0, [0, 255, 0]), [255, 0, 0]);
        assert_eq!(threshold_color(&[], 5.0, [1, 2, 3]), [1, 2, 3]);
    }

    #[test]
    fn times_and_ranges() {
        let now = 1_792_069_200_000;
        assert_eq!(parse_time_expr("now-1h", now), Some(now - 3_600_000));
        assert_eq!(parse_time_expr("now", now), Some(now));
        assert_eq!(parse_time_expr("now-1h30m", now), Some(now - 5_400_000));
        assert_eq!(parse_time_expr("2026-10-15T12:00:00Z", now), Some(1_792_065_600_000));
        assert_eq!(parse_time_expr("now-xyz", now), None);
        assert_eq!(parse_duration_ms("2w"), Some(1_209_600_000));
        assert_eq!(parse_duration_ms("0s"), None);
        assert_eq!(parse_duration_ms("5"), None);
        assert_eq!(duration_text(3_600_000), "1h");
        assert_eq!(duration_text(90_000), "90s");
        let r = resolve_range(&TimeSpec { from: "now-6h".into(), to: "now".into() }, now);
        assert_eq!((r.from_ms, r.to_ms), (now - 21_600_000, now));
        let bad = resolve_range(&TimeSpec { from: "now".into(), to: "now-1h".into() }, now);
        assert!(bad.from_ms < bad.to_ms);
    }

    #[test]
    fn legend_formats() {
        let l: BTreeMap<String, String> = [("instance".to_string(), "a:9100".to_string())].into();
        assert_eq!(series_label("{{instance}} cpu", "x", &l), "a:9100 cpu");
        assert_eq!(series_label("", "x", &l), "x");
        assert_eq!(series_label("{{nope}}", "x", &l), "x");
        assert_eq!(series_label("{{instance", "x", &l), "{{instance");
    }

    #[test]
    fn json_round_trip_and_sanitize() {
        let mut d = d_with(3);
        d.panels[1].options.thresholds = vec![Threshold { value: 1.0, color: [1, 2, 3] }];
        let j = serde_json::to_string_pretty(&d).unwrap();
        let back: Dashboard = serde_json::from_str(&j).unwrap();
        assert_eq!(d, back);
        // a minimal hand-written document is accepted
        let mini: Dashboard = serde_json::from_str(r#"{"id":"x","title":"T","panels":[{"id":"a","title":"A","grid":{"x":0,"y":0,"w":50,"h":0}},{"id":"a","title":"B","grid":{"x":0,"y":0,"w":4,"h":4}}]}"#).unwrap();
        let mut m = mini;
        m.sanitize();
        assert!(no_overlap(&m));
        assert_ne!(m.panels[0].id, m.panels[1].id);
        assert!(serde_json::from_str::<Dashboard>("{}").is_err());
        assert_eq!(Visual::parse("stacked bar"), Some(Visual::StackedBar));
        assert_eq!(Visual::parse("timeseries"), Some(Visual::Line));
        assert_eq!(Visual::parse("x"), None);
        assert_eq!(Unit::parse("%"), Some(Unit::Percent));
    }
}
