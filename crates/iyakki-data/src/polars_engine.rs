//! The Polars engine (`polars` feature): the pipeline becomes a lazy query, so Polars plans it (projection and
//! predicate push-down, parallel CSV parsing) and nothing runs until a page, a count or an export asks.
//!
//! Results of a pipeline that sorts, filters or groups are kept (a few at a time) so scrolling through a
//! result does not run the query for every page.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use polars::prelude::*;
use serde_json::Value;

use crate::expr::{self, BinOp, Expr as E, Lit};
use crate::model::*;
use crate::model::Column;

enum Source {
    Lazy(LazyFrame),
    Frame(Arc<DataFrame>),
}

struct Dataset {
    src: Source,
    /// The whole file once parsed (for scrolling deep into it); the first rows are read without it.
    loaded: Mutex<Option<Arc<DataFrame>>>,
    rows: u64,
    bytes: u64,
    path: String,
}

impl Dataset {
    fn lazy(&self) -> LazyFrame {
        if let Some(f) = self.loaded.lock().unwrap().as_ref() {
            return f.as_ref().clone().lazy();
        }
        self.lazy_unloaded()
    }
}

/// Pages that end within this many rows are read from the file directly; going deeper parses the whole file once.
const HEAD_ROWS: u64 = 2_000;

impl Dataset {
    fn load(&self) -> Result<()> {
        let mut g = self.loaded.lock().unwrap();
        if g.is_none() && matches!(self.src, Source::Lazy(_)) {
            *g = Some(Arc::new(self.lazy_unloaded().collect().map_err(pl)?));
        }
        Ok(())
    }

    fn lazy_unloaded(&self) -> LazyFrame {
        match &self.src {
            Source::Lazy(l) => l.clone(),
            Source::Frame(f) => f.as_ref().clone().lazy(),
        }
    }
}

pub struct PolarsEngine {
    sets: Mutex<HashMap<String, Arc<Dataset>>>,
    next: AtomicU64,
    results: Mutex<Vec<(String, Arc<DataFrame>)>>,
}

impl Default for PolarsEngine {
    fn default() -> Self {
        PolarsEngine::new()
    }
}

fn pl(e: PolarsError) -> DataError {
    let t = e.to_string();
    let t = t.lines().next().unwrap_or("").to_string();
    if t.contains("No such file") || t.contains("os error 2") {
        return DataError::Io("no such file".into());
    }
    DataError::Parse(t)
}

fn dtype_of(d: &DataType) -> DType {
    match d {
        DataType::Boolean => DType::Bool,
        DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64 | DataType::UInt8 | DataType::UInt16 | DataType::UInt32 | DataType::UInt64 => DType::Int,
        DataType::Float32 | DataType::Float64 => DType::Float,
        _ => DType::Str,
    }
}

fn columns_of(lf: &mut LazyFrame) -> Result<Vec<Column>> {
    let schema = lf.collect_schema().map_err(pl)?;
    Ok(schema.iter().map(|(n, d)| Column { name: n.to_string(), dtype: dtype_of(d) }).collect())
}

/// Anything that is not a plain number, text or bool becomes text so every cell has one of the four types.
fn normalize(mut lf: LazyFrame) -> Result<LazyFrame> {
    let schema = lf.collect_schema().map_err(pl)?;
    let casts: Vec<Expr> = schema
        .iter()
        .filter(|(_, d)| dtype_of(d) == DType::Str && !matches!(d, DataType::String))
        .map(|(n, _)| col(n.as_str()).cast(DataType::String))
        .collect();
    Ok(if casts.is_empty() { lf } else { lf.with_columns(casts) })
}

fn to_pl(e: &E, cols: &[Column]) -> Result<Expr> {
    Ok(match e {
        E::Lit(Lit::Null) => lit(NULL),
        E::Lit(Lit::Bool(b)) => lit(*b),
        E::Lit(Lit::Int(i)) => lit(*i),
        E::Lit(Lit::Float(f)) => lit(*f),
        E::Lit(Lit::Str(s)) => lit(s.as_str()),
        E::Col(n) => col(expr::resolve(cols, n)?.as_str()),
        E::Neg(x) => to_pl(x, cols)? * lit(-1),
        E::Not(x) => to_pl(x, cols)?.not(),
        E::IsNull(x, neg) => {
            let x = to_pl(x, cols)?;
            if *neg { x.is_not_null() } else { x.is_null() }
        }
        E::In(x, items) => {
            let x = to_pl(x, cols)?;
            let mut it = items.iter();
            let first = to_pl(it.next().ok_or_else(|| DataError::BadRequest("in needs a list".into()))?, cols)?;
            let mut acc = x.clone().eq(first);
            for i in it {
                acc = acc.or(x.clone().eq(to_pl(i, cols)?));
            }
            acc
        }
        E::Bin(op, a, b) => {
            let (l, r) = (to_pl(a, cols)?, to_pl(b, cols)?);
            match op {
                BinOp::Add => l + r,
                BinOp::Sub => l - r,
                BinOp::Mul => l * r,
                BinOp::Div => {
                    let r = r.cast(DataType::Float64);
                    when(r.clone().neq(lit(0.0))).then(l.cast(DataType::Float64) / r).otherwise(lit(NULL))
                }
                BinOp::Rem => when(r.clone().neq(lit(0))).then(l % r).otherwise(lit(NULL)),
                BinOp::Eq => l.eq(r),
                BinOp::Ne => l.neq(r),
                BinOp::Lt => l.lt(r),
                BinOp::Le => l.lt_eq(r),
                BinOp::Gt => l.gt(r),
                BinOp::Ge => l.gt_eq(r),
                BinOp::And => l.and(r),
                BinOp::Or => l.or(r),
            }
        }
        E::Call(f, args) => {
            let a: Vec<Expr> = args.iter().map(|x| to_pl(x, cols)).collect::<Result<_>>()?;
            let mut a = a.into_iter();
            let first = a.next().unwrap_or_else(|| lit(NULL));
            match f.as_str() {
                "contains" => first.str().to_lowercase().str().contains_literal(a.next().unwrap_or_else(|| lit("")).str().to_lowercase()),
                "starts_with" => first.str().to_lowercase().str().starts_with(a.next().unwrap_or_else(|| lit("")).str().to_lowercase()),
                "ends_with" => first.str().to_lowercase().str().ends_with(a.next().unwrap_or_else(|| lit("")).str().to_lowercase()),
                "lower" => first.str().to_lowercase(),
                "upper" => first.str().to_uppercase(),
                "trim" => first.str().strip_chars(lit(NULL)),
                "len" => first.str().len_chars().cast(DataType::Int64),
                "abs" => first.abs(),
                "floor" => first.floor(),
                "ceil" => first.ceil(),
                "round" => {
                    let d = match args.get(1) {
                        Some(E::Lit(Lit::Int(n))) => (*n).clamp(0, 12) as u32,
                        Some(E::Lit(Lit::Float(n))) => n.clamp(0.0, 12.0) as u32,
                        None => 0,
                        _ => return Err(DataError::BadRequest("round: the number of decimals must be a plain number".into())),
                    };
                    first.cast(DataType::Float64).round(d, RoundMode::default())
                }
                "is_null" => first.is_null(),
                "concat" => {
                    let mut all = vec![first.cast(DataType::String)];
                    all.extend(a.map(|x| x.cast(DataType::String)));
                    concat_str(all, "", true)
                }
                "coalesce" => {
                    let mut all = vec![first];
                    all.extend(a);
                    coalesce(&all)
                }
                other => return Err(DataError::BadRequest(format!("unknown function {other}"))),
            }
        }
    })
}

fn parse_checked(src: &str, cols: &[Column]) -> Result<(E, expr::Ty)> {
    let e = expr::parse(src)?;
    let t = expr::check(&e, cols)?;
    Ok((e, t))
}

fn json_of(v: AnyValue) -> Value {
    match v {
        AnyValue::Null => Value::Null,
        AnyValue::Boolean(b) => b.into(),
        AnyValue::Int8(i) => i.into(),
        AnyValue::Int16(i) => i.into(),
        AnyValue::Int32(i) => i.into(),
        AnyValue::Int64(i) => i.into(),
        AnyValue::UInt8(i) => i.into(),
        AnyValue::UInt16(i) => i.into(),
        AnyValue::UInt32(i) => i.into(),
        AnyValue::UInt64(i) => i.into(),
        AnyValue::Float32(f) => serde_json::Number::from_f64(f as f64).map_or(Value::Null, Value::Number),
        AnyValue::Float64(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
        AnyValue::String(s) => s.into(),
        AnyValue::StringOwned(s) => s.as_str().into(),
        other => other.to_string().trim_matches('"').into(),
    }
}

fn rows_of(df: &DataFrame, from: usize, to: usize) -> Result<Vec<Vec<Value>>> {
    let cols = df.columns();
    let mut out = Vec::with_capacity(to.saturating_sub(from));
    for i in from..to {
        let mut row = Vec::with_capacity(cols.len());
        for c in cols {
            row.push(json_of(c.get(i).map_err(pl)?));
        }
        out.push(row);
    }
    Ok(out)
}

fn f64_of(df: &DataFrame, name: &str) -> Option<f64> {
    match df.column(name).ok()?.get(0).ok()? {
        AnyValue::Null => None,
        v => v.extract::<f64>().filter(|f| f.is_finite()),
    }
}

impl PolarsEngine {
    pub fn new() -> PolarsEngine {
        PolarsEngine { sets: Mutex::new(HashMap::new()), next: AtomicU64::new(1), results: Mutex::new(Vec::new()) }
    }

    fn set(&self, h: &DatasetHandle) -> Result<Arc<Dataset>> {
        self.sets.lock().unwrap().get(&h.0).cloned().ok_or_else(|| DataError::NoSuchDataset(h.0.clone()))
    }

    fn add(&self, d: Dataset) -> DatasetHandle {
        let h = format!("d{}", self.next.fetch_add(1, Ordering::Relaxed));
        self.sets.lock().unwrap().insert(h.clone(), Arc::new(d));
        DatasetHandle(h)
    }

    /// The lazy query of a pipeline and the columns it ends with.
    fn plan(&self, p: &Pipeline) -> Result<(LazyFrame, Vec<Column>)> {
        let mut lf = self.set(&p.handle)?.lazy();
        let mut cols = columns_of(&mut lf)?;
        for op in &p.ops {
            lf = match op {
                Op::Filter { expr: src } => {
                    let (e, t) = parse_checked(src, &cols)?;
                    if t != expr::Ty::T(DType::Bool) {
                        return Err(DataError::BadRequest(format!("the filter {src:?} is not a true/false condition")));
                    }
                    lf.filter(to_pl(&e, &cols)?)
                }
                Op::Sort { keys } => {
                    if keys.is_empty() {
                        return Err(DataError::BadRequest("sort needs at least one column".into()));
                    }
                    let mut exprs = Vec::new();
                    let mut desc = Vec::new();
                    for k in keys {
                        exprs.push(col(expr::resolve(&cols, &k.column)?.as_str()));
                        desc.push(k.dir == Dir::Desc);
                    }
                    lf.sort_by_exprs(exprs, SortMultipleOptions::default().with_order_descending_multi(desc).with_nulls_last(true).with_maintain_order(true))
                }
                Op::Select { columns } => {
                    if columns.is_empty() {
                        return Err(DataError::BadRequest("select needs at least one column".into()));
                    }
                    let names: Vec<String> = columns.iter().map(|c| expr::resolve(&cols, c)).collect::<Result<_>>()?;
                    lf.select(names.iter().map(|n| col(n.as_str())).collect::<Vec<_>>())
                }
                Op::Rename { map } => {
                    let mut old = Vec::new();
                    let mut new = Vec::new();
                    for (o, n) in map {
                        let real = expr::resolve(&cols, o)?;
                        if n.trim().is_empty() {
                            return Err(DataError::BadRequest("a column name cannot be empty".into()));
                        }
                        if cols.iter().any(|c| c.name == *n && c.name != real) {
                            return Err(DataError::BadRequest(format!("there is already a column {n:?}")));
                        }
                        old.push(real);
                        new.push(n.clone());
                    }
                    lf.rename(old, new, true)
                }
                Op::Derive { name, expr: src } => {
                    let (e, _) = parse_checked(src, &cols)?;
                    lf.with_column(to_pl(&e, &cols)?.alias(name.as_str()))
                }
                Op::Limit { n } => lf.limit(*n as u32),
                Op::GroupBy { by, aggs } => {
                    if by.is_empty() && aggs.is_empty() {
                        return Err(DataError::BadRequest("group_by needs columns to group by or something to compute".into()));
                    }
                    let keys: Vec<String> = by.iter().map(|b| expr::resolve(&cols, b)).collect::<Result<_>>()?;
                    let mut ex = Vec::new();
                    for a in aggs {
                        let c = if a.column.is_empty() {
                            if a.func != AggFn::Count {
                                return Err(DataError::BadRequest(format!("{} needs a column", a.func.name())));
                            }
                            None
                        } else {
                            let real = expr::resolve(&cols, &a.column)?;
                            let t = cols.iter().find(|c| c.name == real).map(|c| c.dtype).unwrap_or(DType::Str);
                            if matches!(a.func, AggFn::Sum | AggFn::Mean) && !t.numeric() {
                                return Err(DataError::BadRequest(format!("{} needs a number column, but {} is {}", a.func.name(), a.column, t.name())));
                            }
                            Some(col(real.as_str()))
                        };
                        let e = match (a.func, c) {
                            (AggFn::Count, None) => len().cast(DataType::Int64),
                            (AggFn::Count, Some(c)) => c.count().cast(DataType::Int64),
                            (AggFn::Sum, Some(c)) => c.sum(),
                            (AggFn::Mean, Some(c)) => c.mean(),
                            (AggFn::Min, Some(c)) => c.min(),
                            (AggFn::Max, Some(c)) => c.max(),
                            (AggFn::Unique, Some(c)) => c.drop_nulls().n_unique().cast(DataType::Int64),
                            (AggFn::First, Some(c)) => c.drop_nulls().first(),
                            (AggFn::Last, Some(c)) => c.drop_nulls().last(),
                            (_, None) => unreachable!("checked above"),
                        };
                        ex.push(e.alias(a.out_name().as_str()));
                    }
                    if keys.is_empty() {
                        lf.select(ex)
                    } else {
                        let ks: Vec<Expr> = keys.iter().map(|k| col(k.as_str())).collect();
                        lf.group_by(ks.clone()).agg(ex).sort_by_exprs(ks, SortMultipleOptions::default().with_nulls_last(true).with_maintain_order(true))
                    }
                }
                Op::Join { other, on, kind } => {
                    if on.is_empty() {
                        return Err(DataError::BadRequest("join needs the columns to match on".into()));
                    }
                    let mut right = self.set(other)?.lazy();
                    let rcols = columns_of(&mut right)?;
                    let mut left_on = Vec::new();
                    let mut right_on = Vec::new();
                    for c in on {
                        left_on.push(col(expr::resolve(&cols, c)?.as_str()));
                        right_on.push(col(expr::resolve(&rcols, c)?.as_str()));
                    }
                    let how = if *kind == JoinKind::Left { JoinType::Left } else { JoinType::Inner };
                    let mut args = JoinArgs::new(how).with_suffix(Some("_right".into()));
                    args.maintain_order = MaintainOrderJoin::Left;
                    lf.join(right, left_on, right_on, args)
                }
            };
            lf = normalize(lf)?;
            cols = columns_of(&mut lf)?;
        }
        Ok((lf, cols))
    }

    /// The whole result of a pipeline, kept for the next page.
    fn frame(&self, p: &Pipeline) -> Result<Arc<DataFrame>> {
        let key = format!("{}|{}", p.handle.0, serde_json::to_string(&p.ops).unwrap_or_default());
        if let Some((_, f)) = self.results.lock().unwrap().iter().find(|(k, _)| *k == key) {
            return Ok(f.clone());
        }
        let (lf, _) = self.plan(p)?;
        let df = Arc::new(lf.collect().map_err(pl)?);
        let mut r = self.results.lock().unwrap();
        if r.len() >= 3 {
            r.remove(0);
        }
        r.push((key, df.clone()));
        Ok(df)
    }

    fn info(&self, h: &DatasetHandle, source: String, cols: Vec<Column>, rows: u64, bytes: u64) -> DatasetInfo {
        DatasetInfo { handle: h.clone(), source, columns: cols, rows, bytes, engine: "polars".into() }
    }
}

impl DataService for PolarsEngine {
    fn engine(&self) -> &'static str {
        "polars"
    }

    fn open(&self, req: &OpenRequest) -> Result<DatasetInfo> {
        let meta = std::fs::metadata(&req.path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => DataError::Io(format!("{}: no such file", req.path)),
            std::io::ErrorKind::PermissionDenied => DataError::Io(format!("{}: permission denied", req.path)),
            _ => DataError::Io(format!("{}: {e}", req.path)),
        })?;
        if !meta.is_file() {
            return Err(DataError::Io(format!("{} is not a file", req.path)));
        }
        if meta.len() == 0 {
            return Err(DataError::Parse(format!("{} is empty", req.path)));
        }
        let fmt = req.format.or_else(|| Format::from_path(&req.path)).unwrap_or(Format::Csv);
        let path = PlRefPath::new(req.path.as_str());
        let mut lf = match fmt {
            Format::Jsonl => LazyJsonLineReader::new(path).with_infer_schema_length(std::num::NonZeroUsize::new(5000)).finish().map_err(pl)?,
            _ => {
                let delim = req.delimiter.map_or(fmt.delimiter(), |c| if c.is_ascii() { c as u8 } else { b',' });
                LazyCsvReader::new(path)
                    .with_has_header(req.header)
                    .with_separator(delim)
                    .with_infer_schema_length(None)
                    .with_try_parse_dates(false)
                    .with_missing_is_null(true)
                    .with_truncate_ragged_lines(true)
                    .finish()
                    .map_err(pl)?
            }
        };
        lf = normalize(lf)?;
        let cols = columns_of(&mut lf)?;
        let n = lf.clone().select([len().cast(DataType::Int64)]).collect().map_err(pl)?;
        let rows = match n.column("len").map_err(pl)?.get(0).map_err(pl)? {
            AnyValue::Int64(i) => i.max(0) as u64,
            _ => 0,
        };
        let h = self.add(Dataset { src: Source::Lazy(lf), loaded: Mutex::new(None), rows, bytes: meta.len(), path: req.path.clone() });
        Ok(self.info(&h, req.path.clone(), cols, rows, meta.len()))
    }

    fn close(&self, h: &DatasetHandle) {
        self.sets.lock().unwrap().remove(&h.0);
        let prefix = format!("{}|", h.0);
        self.results.lock().unwrap().retain(|(k, _)| !k.starts_with(&prefix));
    }

    fn schema(&self, p: &Pipeline) -> Result<DatasetInfo> {
        let set = self.set(&p.handle)?;
        if p.ops.is_empty() {
            let (_, cols) = self.plan(p)?;
            return Ok(self.info(&p.handle, set.path.clone(), cols, set.rows, set.bytes));
        }
        let df = self.frame(p)?;
        let (_, cols) = self.plan(p)?;
        Ok(self.info(&p.handle, p.describe(), cols, df.height() as u64, 0))
    }

    fn page(&self, req: &PageRequest) -> Result<Page> {
        let p = &req.pipeline;
        let set = self.set(&p.handle)?;
        if p.ops.is_empty() {
            // the plain file: only the rows asked for are read
            let start = req.offset.min(set.rows);
            let n = req.limit.min(set.rows - start);
            if start + n > HEAD_ROWS {
                set.load()?;
            }
            let (lf, cols) = self.plan(p)?;
            let df = lf.slice(start as i64, n as u32).collect().map_err(pl)?;
            let rows = rows_of(&df, 0, df.height())?;
            return Ok(Page { columns: cols, rows, offset: start, total: set.rows });
        }
        let df = self.frame(p)?;
        let (_, cols) = self.plan_columns(p)?;
        let total = df.height();
        let start = (req.offset as usize).min(total);
        let end = (start + req.limit as usize).min(total);
        Ok(Page { columns: cols, rows: rows_of(&df, start, end)?, offset: start as u64, total: total as u64 })
    }

    fn describe(&self, p: &Pipeline) -> Result<Summary> {
        let df = self.frame(p)?;
        let (_, cols) = self.plan_columns(p)?;
        let mut out = Vec::new();
        for (i, c) in cols.iter().enumerate() {
            let name = c.name.as_str();
            let mut ex = vec![
                col(name).count().cast(DataType::Int64).alias("count"),
                col(name).null_count().cast(DataType::Int64).alias("nulls"),
                col(name).drop_nulls().n_unique().cast(DataType::Int64).alias("unique"),
                col(name).min().alias("min"),
                col(name).max().alias("max"),
            ];
            if c.dtype.numeric() {
                ex.push(col(name).cast(DataType::Float64).mean().alias("mean"));
                ex.push(col(name).cast(DataType::Float64).std(1).alias("std"));
                ex.push(col(name).cast(DataType::Float64).median().alias("median"));
            }
            let one = df.as_ref().clone().lazy().select(ex).collect().map_err(pl)?;
            let int = |n: &str| match one.column(n).ok().and_then(|c| c.get(0).ok()) {
                Some(AnyValue::Int64(i)) => i.max(0) as u64,
                _ => 0,
            };
            let mut top = Vec::new();
            if !c.dtype.numeric() {
                let counts = df.column(name).map_err(pl)?.as_materialized_series().value_counts(true, false, "n".into(), false).map_err(pl)?;
                let counts = counts.sort(["n", name], SortMultipleOptions::default().with_order_descending_multi([true, false])).map_err(pl)?;
                for r in 0..counts.height().min(5) {
                    let v = json_of(counts.column(name).map_err(pl)?.get(r).map_err(pl)?);
                    let n = match counts.column("n").map_err(pl)?.get(r).map_err(pl)? {
                        AnyValue::UInt32(n) => n as u64,
                        AnyValue::Int64(n) => n as u64,
                        AnyValue::UInt64(n) => n,
                        _ => 0,
                    };
                    if v != Value::Null {
                        top.push((crate::table::cell_text(&v), n));
                    }
                }
            }
            let _ = i;
            out.push(ColumnStats {
                name: c.name.clone(),
                dtype: c.dtype,
                count: int("count"),
                nulls: int("nulls"),
                unique: Some(int("unique")),
                min: one.column("min").ok().and_then(|c| c.get(0).ok()).map(json_of).filter(|v| !v.is_null()),
                max: one.column("max").ok().and_then(|c| c.get(0).ok()).map(json_of).filter(|v| !v.is_null()),
                mean: f64_of(&one, "mean"),
                std: f64_of(&one, "std"),
                median: f64_of(&one, "median"),
                top,
            });
        }
        Ok(Summary { rows: df.height() as u64, columns: out })
    }

    fn materialize(&self, p: &Pipeline) -> Result<DatasetInfo> {
        let df = self.frame(p)?;
        let (_, cols) = self.plan_columns(p)?;
        let rows = df.height() as u64;
        let h = self.add(Dataset { src: Source::Frame(df), loaded: Mutex::new(None), rows, bytes: 0, path: p.describe() });
        Ok(self.info(&h, p.describe(), cols, rows, 0))
    }

    fn export_csv(&self, req: &ExportRequest) -> Result<ExportInfo> {
        let df = self.frame(&req.pipeline)?;
        let f = std::fs::File::create(&req.path).map_err(|e| DataError::Io(format!("{}: {e}", req.path)))?;
        let mut out = df.as_ref().clone();
        CsvWriter::new(f).include_header(true).finish(&mut out).map_err(pl)?;
        let bytes = std::fs::metadata(&req.path).map(|m| m.len()).unwrap_or(0);
        Ok(ExportInfo { path: req.path.clone(), rows: df.height() as u64, bytes })
    }
}

impl PolarsEngine {
    /// The columns a pipeline ends with.
    fn plan_columns(&self, p: &Pipeline) -> Result<(LazyFrame, Vec<Column>)> {
        self.plan(p)
    }
}
