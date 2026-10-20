//! The request and result types of the data service. Everything here serializes with serde (JSON), so the same
//! structs can later travel over a remote API unchanged.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A dataset the service holds open (a file, or the result of an earlier [`Pipeline`] kept with `materialize`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DatasetHandle(pub String);

impl std::fmt::Display for DatasetHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The file formats that can be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Csv,
    Tsv,
    /// One JSON object per line (needs the `polars` engine).
    Jsonl,
}

impl Format {
    /// The format a file name suggests (`None` for names that are not tabular data).
    pub fn from_path(p: &str) -> Option<Format> {
        let l = p.to_ascii_lowercase();
        if l.ends_with(".csv") || l.ends_with(".txt") {
            Some(Format::Csv)
        } else if l.ends_with(".tsv") || l.ends_with(".tab") {
            Some(Format::Tsv)
        } else if l.ends_with(".jsonl") || l.ends_with(".ndjson") {
            Some(Format::Jsonl)
        } else {
            None
        }
    }

    pub fn delimiter(self) -> u8 {
        if self == Format::Tsv { b'\t' } else { b',' }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenRequest {
    pub path: String,
    /// Left out: taken from the file name.
    #[serde(default)]
    pub format: Option<Format>,
    /// Left out: `,` for CSV, tab for TSV.
    #[serde(default)]
    pub delimiter: Option<char>,
    /// The first row names the columns (default true).
    #[serde(default = "yes")]
    pub header: bool,
}

fn yes() -> bool {
    true
}

impl OpenRequest {
    pub fn new(path: impl Into<String>) -> OpenRequest {
        OpenRequest { path: path.into(), format: None, delimiter: None, header: true }
    }
}

/// The type of a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DType {
    Int,
    Float,
    Bool,
    Str,
}

impl DType {
    pub fn name(self) -> &'static str {
        match self {
            DType::Int => "int",
            DType::Float => "float",
            DType::Bool => "bool",
            DType::Str => "str",
        }
    }

    pub fn numeric(self) -> bool {
        matches!(self, DType::Int | DType::Float)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub dtype: DType,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatasetInfo {
    pub handle: DatasetHandle,
    /// Where it came from (a path), or the description of the pipeline that made it.
    pub source: String,
    pub columns: Vec<Column>,
    pub rows: u64,
    /// Bytes of the file (0 for a derived dataset).
    pub bytes: u64,
    /// Which engine answers: "polars" or "builtin".
    pub engine: String,
}

// ------------------------------------------------------------------ the pipeline

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Asc,
    Desc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortKey {
    pub column: String,
    pub dir: Dir,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AggFn {
    Count,
    Sum,
    Mean,
    Min,
    Max,
    /// Number of different values.
    Unique,
    First,
    Last,
}

impl AggFn {
    pub fn parse(s: &str) -> Option<AggFn> {
        Some(match s.to_ascii_lowercase().as_str() {
            "count" | "n" | "len" => AggFn::Count,
            "sum" | "total" => AggFn::Sum,
            "mean" | "avg" | "average" => AggFn::Mean,
            "min" => AggFn::Min,
            "max" => AggFn::Max,
            "unique" | "nunique" | "distinct" => AggFn::Unique,
            "first" => AggFn::First,
            "last" => AggFn::Last,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            AggFn::Count => "count",
            AggFn::Sum => "sum",
            AggFn::Mean => "mean",
            AggFn::Min => "min",
            AggFn::Max => "max",
            AggFn::Unique => "unique",
            AggFn::First => "first",
            AggFn::Last => "last",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agg {
    /// The column to aggregate (ignored by `count`, which may leave it empty).
    #[serde(default)]
    pub column: String,
    pub func: AggFn,
    /// The result column's name (default `<func>_<column>`, or `count`).
    #[serde(default, rename = "as")]
    pub alias: Option<String>,
}

impl Agg {
    pub fn out_name(&self) -> String {
        match (&self.alias, self.func) {
            (Some(a), _) if !a.is_empty() => a.clone(),
            (_, AggFn::Count) if self.column.is_empty() => "count".to_string(),
            _ => format!("{}_{}", self.func.name(), self.column),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JoinKind {
    Inner,
    Left,
}

/// One step of a query. The steps run in order; the result of one is the input of the next.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Keep the rows where the expression is true: `price > 10 and contains(name, "tea")`.
    Filter { expr: String },
    Sort { keys: Vec<SortKey> },
    /// Keep (and order) these columns.
    Select { columns: Vec<String> },
    /// `{old: new}` pairs.
    Rename { map: Vec<(String, String)> },
    /// Add or replace a column computed from an expression: `total = price * qty`.
    Derive { name: String, expr: String },
    GroupBy { by: Vec<String>, aggs: Vec<Agg> },
    /// Join with another open dataset on the columns of the same name (needs the `polars` engine).
    Join { other: DatasetHandle, on: Vec<String>, kind: JoinKind },
    /// Only the first `n` rows.
    Limit { n: u64 },
}

/// A dataset and the steps applied to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pipeline {
    pub handle: DatasetHandle,
    #[serde(default)]
    pub ops: Vec<Op>,
}

impl Pipeline {
    pub fn new(handle: &DatasetHandle) -> Pipeline {
        Pipeline { handle: handle.clone(), ops: Vec::new() }
    }

    pub fn with(mut self, op: Op) -> Pipeline {
        self.ops.push(op);
        self
    }

    /// A short readable description ("filter price > 10 → sort name").
    pub fn describe(&self) -> String {
        let parts: Vec<String> = self
            .ops
            .iter()
            .map(|o| match o {
                Op::Filter { expr } => format!("filter {expr}"),
                Op::Sort { keys } => format!("sort {}", keys.iter().map(|k| format!("{}{}", k.column, if k.dir == Dir::Desc { " desc" } else { "" })).collect::<Vec<_>>().join(", ")),
                Op::Select { columns } => format!("select {}", columns.join(", ")),
                Op::Rename { map } => format!("rename {}", map.iter().map(|(a, b)| format!("{a}→{b}")).collect::<Vec<_>>().join(", ")),
                Op::Derive { name, expr } => format!("{name} = {expr}"),
                Op::GroupBy { by, aggs } => format!("group by {} ({})", by.join(", "), aggs.iter().map(Agg::out_name).collect::<Vec<_>>().join(", ")),
                Op::Join { other, on, .. } => format!("join {other} on {}", on.join(", ")),
                Op::Limit { n } => format!("first {n}"),
            })
            .collect();
        parts.join(" → ")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageRequest {
    #[serde(flatten)]
    pub pipeline: Pipeline,
    #[serde(default)]
    pub offset: u64,
    pub limit: u64,
}

/// Rows of a result. Cells are JSON values: null, number, bool or string.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    pub offset: u64,
    /// Rows in the whole result (after the filters), not just this page.
    pub total: u64,
}

/// Statistics of one column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnStats {
    pub name: String,
    pub dtype: DType,
    pub count: u64,
    pub nulls: u64,
    pub unique: Option<u64>,
    pub min: Option<Value>,
    pub max: Option<Value>,
    pub mean: Option<f64>,
    pub std: Option<f64>,
    pub median: Option<f64>,
    /// The most frequent values with how often (text and bool columns).
    #[serde(default)]
    pub top: Vec<(String, u64)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub rows: u64,
    pub columns: Vec<ColumnStats>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportRequest {
    #[serde(flatten)]
    pub pipeline: Pipeline,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportInfo {
    pub path: String,
    pub rows: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum DataError {
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Parse(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NoSuchColumn(String),
    #[error("no dataset {0} is open")]
    NoSuchDataset(String),
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, DataError>;

/// The data service: open files, look at them and ask questions of them. Every call is a plain request struct and a
/// plain result, so an implementation can sit behind a remote API.
pub trait DataService: Send + Sync {
    /// "polars" or "builtin".
    fn engine(&self) -> &'static str;
    fn open(&self, req: &OpenRequest) -> Result<DatasetInfo>;
    fn close(&self, h: &DatasetHandle);
    /// The columns and row count of a dataset after the steps of the pipeline.
    fn schema(&self, p: &Pipeline) -> Result<DatasetInfo>;
    fn page(&self, req: &PageRequest) -> Result<Page>;
    /// Statistics of every column of the result.
    fn describe(&self, p: &Pipeline) -> Result<Summary>;
    /// Keep the result of a pipeline as a dataset of its own (to join with, or to explore further).
    fn materialize(&self, p: &Pipeline) -> Result<DatasetInfo>;
    fn export_csv(&self, req: &ExportRequest) -> Result<ExportInfo>;
}
