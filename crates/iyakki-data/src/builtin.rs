//! The built-in engine: CSV/TSV files on disk, no dependencies. It keeps only a table of where each row starts and
//! loads a column into memory only when a filter, sort or statistic needs it, so a file of a million rows is paged
//! without being held in memory. Used when the `polars` feature is off (the Buildroot image), and kept as the
//! reference the Polars engine is tested against.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::csv::{self, Records};
use crate::expr::{self, columns_used, V};
use crate::model::*;

enum Base {
    File(FileSrc),
    Mem(MemTable),
}

struct FileSrc {
    path: String,
    delim: u8,
    cols: Vec<Column>,
    offsets: Vec<u64>,
    bytes: u64,
    cache: Mutex<HashMap<usize, Arc<Vec<V>>>>,
}

struct MemTable {
    cols: Vec<Column>,
    data: Vec<Arc<Vec<V>>>,
    n: usize,
}

fn to_v(s: &str, t: DType) -> V {
    if s.is_empty() {
        return V::Null;
    }
    match t {
        DType::Int => s.parse().map_or(V::Null, V::Int),
        DType::Float => s.parse().map_or(V::Null, V::Float),
        DType::Bool => V::Bool(s == "true"),
        DType::Str => V::Str(s.to_string()),
    }
}

fn io(e: std::io::Error, path: &str) -> DataError {
    DataError::Io(match e.kind() {
        std::io::ErrorKind::NotFound => format!("{path}: no such file"),
        std::io::ErrorKind::PermissionDenied => format!("{path}: permission denied"),
        _ => format!("{path}: {e}"),
    })
}

impl Base {
    fn cols(&self) -> &[Column] {
        match self {
            Base::File(f) => &f.cols,
            Base::Mem(m) => &m.cols,
        }
    }

    fn n(&self) -> usize {
        match self {
            Base::File(f) => f.offsets.len(),
            Base::Mem(m) => m.n,
        }
    }

    /// One whole column, loaded once and kept.
    fn column(&self, i: usize) -> Result<Arc<Vec<V>>> {
        match self {
            Base::Mem(m) => Ok(m.data[i].clone()),
            Base::File(f) => {
                if let Some(c) = f.cache.lock().unwrap().get(&i) {
                    return Ok(c.clone());
                }
                let t = f.cols[i].dtype;
                if f.offsets.is_empty() {
                    return Ok(Arc::new(Vec::new()));
                }
                let mut r = Records::open(&f.path, f.delim).map_err(|e| io(e, &f.path))?;
                let mut rec = Vec::new();
                let mut out = Vec::with_capacity(f.offsets.len());
                // skip to the first data row (the header, when there is one, lies before it)
                r.seek(f.offsets.first().copied().unwrap_or(0)).map_err(|e| io(e, &f.path))?;
                while r.next(&mut rec).map_err(|e| io(e, &f.path))?.is_some() {
                    out.push(rec.get(i).map_or(V::Null, |s| to_v(s, t)));
                }
                let c = Arc::new(out);
                let mut cache = f.cache.lock().unwrap();
                if cache.len() >= 12 {
                    cache.clear();
                }
                cache.insert(i, c.clone());
                Ok(c)
            }
        }
    }

    /// The cells of some rows of some columns, row by row.
    fn rows(&self, ids: &[u32], cols: &[usize]) -> Result<Vec<Vec<V>>> {
        match self {
            Base::Mem(m) => Ok(ids.iter().map(|&r| cols.iter().map(|&c| m.data[c][r as usize].clone()).collect()).collect()),
            Base::File(f) => {
                let cached: Vec<Option<Arc<Vec<V>>>> = {
                    let c = f.cache.lock().unwrap();
                    cols.iter().map(|i| c.get(i).cloned()).collect()
                };
                let mut r = Records::open(&f.path, f.delim).map_err(|e| io(e, &f.path))?;
                let mut rec = Vec::new();
                let mut out = Vec::with_capacity(ids.len());
                for &id in ids {
                    let mut row = Vec::with_capacity(cols.len());
                    if cached.iter().all(Option::is_some) && !cols.is_empty() {
                        for (k, _) in cols.iter().enumerate() {
                            row.push(cached[k].as_ref().map_or(V::Null, |c| c[id as usize].clone()));
                        }
                    } else {
                        r.seek(f.offsets[id as usize]).map_err(|e| io(e, &f.path))?;
                        r.next(&mut rec).map_err(|e| io(e, &f.path))?;
                        for &c in cols {
                            row.push(rec.get(c).map_or(V::Null, |s| to_v(s, f.cols[c].dtype)));
                        }
                    }
                    out.push(row);
                }
                Ok(out)
            }
        }
    }
}

#[derive(Clone)]
enum ColRef {
    Base(usize),
    /// Computed for every row of the base (indexed by base row).
    Computed(Arc<Vec<V>>),
}

/// A dataset after some steps: which columns are visible and which rows, in what order.
struct View {
    base: Arc<Base>,
    vis: Vec<(Column, ColRef)>,
    rows: Option<Arc<Vec<u32>>>,
    n: u64,
}

impl View {
    fn of(base: Arc<Base>) -> View {
        let vis = base.cols().iter().enumerate().map(|(i, c)| (c.clone(), ColRef::Base(i))).collect();
        let n = base.n() as u64;
        View { base, vis, rows: None, n }
    }

    fn columns(&self) -> Vec<Column> {
        self.vis.iter().map(|v| v.0.clone()).collect()
    }

    fn ids(&self) -> Vec<u32> {
        match &self.rows {
            Some(r) => r.as_ref().clone(),
            None => (0..self.base.n() as u32).collect(),
        }
    }

    fn values(&self, c: &ColRef) -> Result<Arc<Vec<V>>> {
        match c {
            ColRef::Base(i) => self.base.column(*i),
            ColRef::Computed(v) => Ok(v.clone()),
        }
    }

    fn find(&self, name: &str) -> Result<usize> {
        let cols = self.columns();
        let real = expr::resolve(&cols, name)?;
        Ok(cols.iter().position(|c| c.name == real).unwrap_or(0))
    }

    /// The cells of rows `ids` of the visible columns.
    fn cells(&self, ids: &[u32]) -> Result<Vec<Vec<V>>> {
        let mut out: Vec<Vec<V>> = vec![Vec::with_capacity(self.vis.len()); ids.len()];
        let base_cols: Vec<usize> = self.vis.iter().filter_map(|v| if let ColRef::Base(i) = v.1 { Some(i) } else { None }).collect();
        let fetched = if base_cols.is_empty() { vec![vec![]; ids.len()] } else { self.base.rows(ids, &base_cols)? };
        for (r, row) in out.iter_mut().enumerate() {
            let mut k = 0;
            for v in &self.vis {
                match &v.1 {
                    ColRef::Base(_) => {
                        row.push(fetched[r][k].clone());
                        k += 1;
                    }
                    ColRef::Computed(c) => row.push(c[ids[r] as usize].clone()),
                }
            }
        }
        Ok(out)
    }
}

fn cmp_v(a: &V, b: &V) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    match (a, b) {
        (V::Null, V::Null) => Equal,
        (V::Null, _) => Greater, // nulls come last
        (_, V::Null) => Less,
        (V::Int(x), V::Int(y)) => x.cmp(y),
        (V::Str(x), V::Str(y)) => x.cmp(y),
        (V::Bool(x), V::Bool(y)) => x.cmp(y),
        _ => {
            let f = |v: &V| match v {
                V::Int(i) => *i as f64,
                V::Float(f) => *f,
                _ => 0.0,
            };
            f(a).partial_cmp(&f(b)).unwrap_or(Equal)
        }
    }
}

fn is_float(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.')) && s.parse::<f64>().is_ok_and(|f| f.is_finite())
}

/// Read the whole file once: names, types and where each row starts.
fn scan(path: &str, delim: u8, header: bool) -> Result<FileSrc> {
    let meta = std::fs::metadata(path).map_err(|e| io(e, path))?;
    if !meta.is_file() {
        return Err(DataError::Io(format!("{path} is not a file")));
    }
    let mut r = Records::open(path, delim).map_err(|e| io(e, path))?;
    let mut rec = Vec::new();
    let first = r.next(&mut rec).map_err(|e| io(e, path))?;
    if first.is_none() {
        return Err(DataError::Parse(format!("{path} is empty")));
    }
    let width = rec.len();
    let mut names: Vec<String> = if header { rec.iter().map(|s| s.trim().to_string()).collect() } else { (1..=width).map(|i| format!("column_{i}")).collect() };
    for (i, n) in names.iter_mut().enumerate() {
        if n.is_empty() {
            *n = format!("column_{}", i + 1);
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    for n in names.iter_mut() {
        let base = n.clone();
        let mut k = 2;
        while !seen.insert(n.clone()) {
            *n = format!("{base}_{k}");
            k += 1;
        }
    }
    // per column: could it be an int / float / bool, and was anything non-empty seen
    let mut flags = vec![(true, true, true, false); width];
    let mut offsets: Vec<u64> = Vec::new();
    let feed = |rec: &[String], flags: &mut Vec<(bool, bool, bool, bool)>| {
        for (i, f) in flags.iter_mut().enumerate() {
            let s = rec.get(i).map_or("", |s| s.as_str());
            if s.is_empty() {
                continue;
            }
            f.3 = true;
            if f.0 && s.parse::<i64>().is_err() {
                f.0 = false;
            }
            if f.1 && !is_float(s) {
                f.1 = false;
            }
            if f.2 && s != "true" && s != "false" {
                f.2 = false;
            }
        }
    };
    if !header {
        offsets.push(first.unwrap_or(0));
        feed(&rec, &mut flags);
    }
    while let Some(off) = r.next(&mut rec).map_err(|e| io(e, path))? {
        offsets.push(off);
        feed(&rec, &mut flags);
    }
    let cols = names
        .into_iter()
        .zip(flags)
        .map(|(name, f)| Column { name, dtype: if !f.3 { DType::Str } else if f.0 { DType::Int } else if f.1 { DType::Float } else if f.2 { DType::Bool } else { DType::Str } })
        .collect();
    Ok(FileSrc { path: path.to_string(), delim, cols, offsets, bytes: meta.len(), cache: Mutex::new(HashMap::new()) })
}

pub struct Builtin {
    sets: Mutex<HashMap<String, Arc<Base>>>,
    next: AtomicU64,
    views: Mutex<Vec<(String, Arc<View>)>>,
}

impl Default for Builtin {
    fn default() -> Self {
        Builtin::new()
    }
}

impl Builtin {
    pub fn new() -> Builtin {
        Builtin { sets: Mutex::new(HashMap::new()), next: AtomicU64::new(1), views: Mutex::new(Vec::new()) }
    }

    fn base(&self, h: &DatasetHandle) -> Result<Arc<Base>> {
        self.sets.lock().unwrap().get(&h.0).cloned().ok_or_else(|| DataError::NoSuchDataset(h.0.clone()))
    }

    fn add(&self, b: Base) -> DatasetHandle {
        let h = format!("d{}", self.next.fetch_add(1, Ordering::Relaxed));
        self.sets.lock().unwrap().insert(h.clone(), Arc::new(b));
        DatasetHandle(h)
    }

    fn info(&self, h: &DatasetHandle, source: String, v: &View) -> DatasetInfo {
        let bytes = if let Base::File(f) = &*v.base { f.bytes } else { 0 };
        DatasetInfo { handle: h.clone(), source, columns: v.columns(), rows: v.n, bytes, engine: "builtin".into() }
    }

    fn view(&self, p: &Pipeline) -> Result<Arc<View>> {
        let key = format!("{}|{}", p.handle.0, serde_json::to_string(&p.ops).unwrap_or_default());
        if let Some((_, v)) = self.views.lock().unwrap().iter().find(|(k, _)| *k == key) {
            return Ok(v.clone());
        }
        let mut v = View::of(self.base(&p.handle)?);
        for op in &p.ops {
            v = self.apply(v, op)?;
        }
        let v = Arc::new(v);
        let mut cache = self.views.lock().unwrap();
        if cache.len() >= 12 {
            cache.remove(0);
        }
        cache.push((key, v.clone()));
        Ok(v)
    }

    fn apply(&self, mut v: View, op: &Op) -> Result<View> {
        match op {
            Op::Filter { expr: src } => {
                let e = expr::parse(src)?;
                let cols = v.columns();
                if expr::check(&e, &cols)? != expr::Ty::T(DType::Bool) {
                    return Err(DataError::BadRequest(format!("the filter {src:?} is not a true/false condition")));
                }
                let data = self.expr_data(&v, &e)?;
                let keep: Vec<u32> = v
                    .ids()
                    .into_iter()
                    .filter(|&id| matches!(expr::eval(&e, &|n| lookup(&data, n, id)), V::Bool(true)))
                    .collect();
                v.n = keep.len() as u64;
                v.rows = Some(Arc::new(keep));
            }
            Op::Sort { keys } => {
                if keys.is_empty() {
                    return Err(DataError::BadRequest("sort needs at least one column".into()));
                }
                let mut cols = Vec::new();
                for k in keys {
                    let i = v.find(&k.column)?;
                    cols.push((v.values(&v.vis[i].1)?, k.dir));
                }
                let mut ids = v.ids();
                ids.sort_by(|&a, &b| {
                    for (c, dir) in &cols {
                        let (x, y) = (&c[a as usize], &c[b as usize]);
                        let o = if *dir == Dir::Asc || matches!(x, V::Null) || matches!(y, V::Null) { cmp_v(x, y) } else { cmp_v(y, x) };
                        if o.is_ne() {
                            return o;
                        }
                    }
                    std::cmp::Ordering::Equal
                });
                v.rows = Some(Arc::new(ids));
            }
            Op::Select { columns } => {
                if columns.is_empty() {
                    return Err(DataError::BadRequest("select needs at least one column".into()));
                }
                let mut vis = Vec::new();
                for c in columns {
                    let i = v.find(c)?;
                    vis.push(v.vis[i].clone());
                }
                v.vis = vis;
            }
            Op::Rename { map } => {
                for (old, new) in map {
                    let i = v.find(old)?;
                    if new.trim().is_empty() {
                        return Err(DataError::BadRequest("a column name cannot be empty".into()));
                    }
                    if v.vis.iter().enumerate().any(|(j, c)| j != i && c.0.name == *new) {
                        return Err(DataError::BadRequest(format!("there is already a column {new:?}")));
                    }
                    v.vis[i].0.name = new.clone();
                }
            }
            Op::Derive { name, expr: src } => {
                let e = expr::parse(src)?;
                let cols = v.columns();
                let ty = match expr::check(&e, &cols)? {
                    expr::Ty::T(t) => t,
                    expr::Ty::Null => DType::Str,
                };
                let data = self.expr_data(&v, &e)?;
                let n = v.base.n();
                let vals: Vec<V> = (0..n as u32).map(|id| expr::eval(&e, &|c| lookup(&data, c, id))).collect();
                let col = Column { name: name.clone(), dtype: ty };
                match v.vis.iter().position(|c| c.0.name == *name) {
                    Some(i) => v.vis[i] = (col, ColRef::Computed(Arc::new(vals))),
                    None => v.vis.push((col, ColRef::Computed(Arc::new(vals)))),
                }
            }
            Op::Limit { n } => {
                let mut ids = v.ids();
                ids.truncate(*n as usize);
                v.n = ids.len() as u64;
                v.rows = Some(Arc::new(ids));
            }
            Op::GroupBy { by, aggs } => return self.group(&v, by, aggs),
            Op::Join { other, on, kind } => return self.join(&v, other, on, *kind),
        }
        Ok(v)
    }

    /// The columns an expression uses, loaded: (name as written, values by base row).
    fn expr_data(&self, v: &View, e: &expr::Expr) -> Result<Vec<(String, Arc<Vec<V>>)>> {
        let mut used = Vec::new();
        columns_used(e, &mut used);
        used.into_iter()
            .map(|n| {
                let i = v.find(&n)?;
                Ok((n, v.values(&v.vis[i].1)?))
            })
            .collect()
    }

    fn group(&self, v: &View, by: &[String], aggs: &[Agg]) -> Result<View> {
        if by.is_empty() && aggs.is_empty() {
            return Err(DataError::BadRequest("group_by needs columns to group by or something to compute".into()));
        }
        let mut key_cols = Vec::new();
        for b in by {
            let i = v.find(b)?;
            key_cols.push((v.vis[i].0.clone(), v.values(&v.vis[i].1)?));
        }
        struct Spec {
            f: AggFn,
            out: Column,
            vals: Option<Arc<Vec<V>>>,
        }
        let mut specs = Vec::new();
        for a in aggs {
            let (vals, src_t) = if a.column.is_empty() {
                if a.func != AggFn::Count {
                    return Err(DataError::BadRequest(format!("{} needs a column", a.func.name())));
                }
                (None, DType::Int)
            } else {
                let i = v.find(&a.column)?;
                (Some(v.values(&v.vis[i].1)?), v.vis[i].0.dtype)
            };
            if matches!(a.func, AggFn::Sum | AggFn::Mean) && !src_t.numeric() {
                return Err(DataError::BadRequest(format!("{} needs a number column, but {} is {}", a.func.name(), a.column, src_t.name())));
            }
            let dtype = match a.func {
                AggFn::Count | AggFn::Unique => DType::Int,
                AggFn::Mean => DType::Float,
                _ => src_t,
            };
            specs.push(Spec { f: a.func, out: Column { name: a.out_name(), dtype }, vals });
        }
        #[derive(Default, Clone)]
        struct Acc {
            n: u64,
            sum: f64,
            isum: i64,
            min: Option<V>,
            max: Option<V>,
            first: Option<V>,
            last: Option<V>,
            uniq: HashSet<String>,
        }
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut keys: Vec<Vec<V>> = Vec::new();
        let mut accs: Vec<Vec<Acc>> = Vec::new();
        for id in v.ids() {
            let kv: Vec<V> = key_cols.iter().map(|(_, c)| c[id as usize].clone()).collect();
            let ks: String = kv.iter().map(|x| if *x == V::Null { "\u{0}".to_string() } else { x.text() }).collect::<Vec<_>>().join("\u{1f}");
            let g = *index.entry(ks).or_insert_with(|| {
                keys.push(kv);
                accs.push(vec![Acc::default(); specs.len()]);
                keys.len() - 1
            });
            for (s, a) in specs.iter().zip(accs[g].iter_mut()) {
                let Some(vals) = &s.vals else {
                    a.n += 1;
                    continue;
                };
                let x = &vals[id as usize];
                if *x == V::Null {
                    continue;
                }
                a.n += 1;
                match x {
                    V::Int(i) => {
                        a.sum += *i as f64;
                        a.isum = a.isum.saturating_add(*i);
                    }
                    V::Float(f) => a.sum += f,
                    _ => {}
                }
                if a.min.as_ref().is_none_or(|m| cmp_v(x, m).is_lt()) {
                    a.min = Some(x.clone());
                }
                if a.max.as_ref().is_none_or(|m| cmp_v(x, m).is_gt()) {
                    a.max = Some(x.clone());
                }
                if a.first.is_none() {
                    a.first = Some(x.clone());
                }
                a.last = Some(x.clone());
                if s.f == AggFn::Unique {
                    a.uniq.insert(x.text());
                }
            }
        }
        // one row per group, in key order
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_by(|&a, &b| {
            for k in 0..by.len() {
                let o = cmp_v(&keys[a][k], &keys[b][k]);
                if o.is_ne() {
                    return o;
                }
            }
            std::cmp::Ordering::Equal
        });
        let mut cols: Vec<Column> = key_cols.iter().map(|k| k.0.clone()).collect();
        cols.extend(specs.iter().map(|s| s.out.clone()));
        let mut data: Vec<Vec<V>> = vec![Vec::with_capacity(order.len()); cols.len()];
        for &g in &order {
            for (k, kv) in keys[g].iter().enumerate() {
                data[k].push(kv.clone());
            }
            for (j, s) in specs.iter().enumerate() {
                let a = &accs[g][j];
                let int_col = s.out.dtype == DType::Int;
                let val = match s.f {
                    AggFn::Count => V::Int(a.n as i64),
                    AggFn::Unique => V::Int(a.uniq.len() as i64),
                    AggFn::Sum => if int_col { V::Int(a.isum) } else { V::Float(a.sum) },
                    AggFn::Mean => if a.n == 0 { V::Null } else { V::Float(a.sum / a.n as f64) },
                    AggFn::Min => a.min.clone().unwrap_or(V::Null),
                    AggFn::Max => a.max.clone().unwrap_or(V::Null),
                    AggFn::First => a.first.clone().unwrap_or(V::Null),
                    AggFn::Last => a.last.clone().unwrap_or(V::Null),
                };
                data[by.len() + j].push(val);
            }
        }
        let n = order.len();
        Ok(View::of(Arc::new(Base::Mem(MemTable { cols, data: data.into_iter().map(Arc::new).collect(), n }))))
    }

    fn join(&self, v: &View, other: &DatasetHandle, on: &[String], kind: JoinKind) -> Result<View> {
        if on.is_empty() {
            return Err(DataError::BadRequest("join needs the columns to match on".into()));
        }
        let ob = View::of(self.base(other)?);
        let mut lk = Vec::new();
        let mut rk = Vec::new();
        for c in on {
            let i = v.find(c)?;
            lk.push(v.values(&v.vis[i].1)?);
            let j = ob.find(c)?;
            rk.push(ob.values(&ob.vis[j].1)?);
        }
        let key = |cols: &[Arc<Vec<V>>], id: usize| -> Option<String> {
            let mut s = String::new();
            for c in cols {
                if c[id] == V::Null {
                    return None;
                }
                s.push_str(&c[id].text());
                s.push('\u{1f}');
            }
            Some(s)
        };
        let mut map: HashMap<String, Vec<u32>> = HashMap::new();
        for id in 0..ob.base.n() {
            if let Some(k) = key(&rk, id) {
                map.entry(k).or_default().push(id as u32);
            }
        }
        let on_right: Vec<usize> = on.iter().map(|c| ob.find(c)).collect::<Result<_>>()?;
        let right_cols: Vec<usize> = (0..ob.vis.len()).filter(|i| !on_right.contains(i)).collect();
        let mut pairs: Vec<(u32, Option<u32>)> = Vec::new();
        for id in v.ids() {
            match key(&lk, id as usize).and_then(|k| map.get(&k)) {
                Some(rs) => pairs.extend(rs.iter().map(|r| (id, Some(*r)))),
                None if kind == JoinKind::Left => pairs.push((id, None)),
                None => {}
            }
        }
        let mut cols = v.columns();
        let left_names: HashSet<String> = cols.iter().map(|c| c.name.clone()).collect();
        let lids: Vec<u32> = pairs.iter().map(|p| p.0).collect();
        let mut data: Vec<Vec<V>> = vec![Vec::with_capacity(pairs.len()); cols.len()];
        for row in v.cells(&lids)? {
            for (k, x) in row.into_iter().enumerate() {
                data[k].push(x);
            }
        }
        for &rc in &right_cols {
            let mut c = ob.vis[rc].0.clone();
            if left_names.contains(&c.name) {
                c.name = format!("{}_right", c.name);
            }
            cols.push(c);
            let vals = ob.values(&ob.vis[rc].1)?;
            data.push(pairs.iter().map(|p| p.1.map_or(V::Null, |r| vals[r as usize].clone())).collect());
        }
        let n = pairs.len();
        Ok(View::of(Arc::new(Base::Mem(MemTable { cols, data: data.into_iter().map(Arc::new).collect(), n }))))
    }
}

fn lookup(data: &[(String, Arc<Vec<V>>)], name: &str, id: u32) -> V {
    data.iter().find(|d| d.0 == name).map_or(V::Null, |d| d.1[id as usize].clone())
}

fn stats(c: &Column, vals: &[V]) -> ColumnStats {
    let present: Vec<&V> = vals.iter().filter(|v| **v != V::Null).collect();
    let mut s = ColumnStats { name: c.name.clone(), dtype: c.dtype, count: present.len() as u64, nulls: (vals.len() - present.len()) as u64, unique: None, min: None, max: None, mean: None, std: None, median: None, top: vec![] };
    s.min = present.iter().copied().min_by(|a, b| cmp_v(a, b)).map(V::to_json);
    s.max = present.iter().copied().max_by(|a, b| cmp_v(a, b)).map(V::to_json);
    let mut counts: HashMap<String, u64> = HashMap::new();
    for v in &present {
        *counts.entry(v.text()).or_default() += 1;
    }
    s.unique = Some(counts.len() as u64);
    if c.dtype.numeric() {
        let mut f: Vec<f64> = present
            .iter()
            .filter_map(|v| match v {
                V::Int(i) => Some(*i as f64),
                V::Float(f) => Some(*f),
                _ => None,
            })
            .collect();
        if !f.is_empty() {
            let n = f.len() as f64;
            let mean = f.iter().sum::<f64>() / n;
            s.mean = Some(mean);
            s.std = (f.len() > 1).then(|| (f.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt());
            f.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            s.median = Some(if f.len() % 2 == 1 { f[f.len() / 2] } else { (f[f.len() / 2 - 1] + f[f.len() / 2]) / 2.0 });
        }
    } else {
        let mut top: Vec<(String, u64)> = counts.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        top.truncate(5);
        s.top = top;
    }
    s
}

fn json_text(v: &V) -> String {
    v.text()
}

impl DataService for Builtin {
    fn engine(&self) -> &'static str {
        "builtin"
    }

    fn open(&self, req: &OpenRequest) -> Result<DatasetInfo> {
        let fmt = req.format.or_else(|| Format::from_path(&req.path)).unwrap_or(Format::Csv);
        if fmt == Format::Jsonl {
            return Err(DataError::Unsupported("JSON lines files need the full data engine, which this build does not include. Convert the file to CSV.".into()));
        }
        let delim = req.delimiter.map_or(fmt.delimiter(), |c| if c.is_ascii() { c as u8 } else { b',' });
        let f = scan(&req.path, delim, req.header)?;
        let h = self.add(Base::File(f));
        let v = View::of(self.base(&h)?);
        Ok(self.info(&h, req.path.clone(), &v))
    }

    fn close(&self, h: &DatasetHandle) {
        self.sets.lock().unwrap().remove(&h.0);
        let prefix = format!("{}|", h.0);
        self.views.lock().unwrap().retain(|(k, _)| !k.starts_with(&prefix));
    }

    fn schema(&self, p: &Pipeline) -> Result<DatasetInfo> {
        let v = self.view(p)?;
        Ok(self.info(&p.handle, p.describe(), &v))
    }

    fn page(&self, req: &PageRequest) -> Result<Page> {
        let v = self.view(&req.pipeline)?;
        let start = (req.offset as usize).min(v.n as usize);
        let end = (start + req.limit as usize).min(v.n as usize);
        let ids: Vec<u32> = match &v.rows {
            Some(r) => r[start..end].to_vec(),
            None => (start as u32..end as u32).collect(),
        };
        let rows = v.cells(&ids)?.into_iter().map(|r| r.iter().map(V::to_json).collect()).collect();
        Ok(Page { columns: v.columns(), rows, offset: start as u64, total: v.n })
    }

    fn describe(&self, p: &Pipeline) -> Result<Summary> {
        let v = self.view(p)?;
        let ids = v.ids();
        let mut out = Vec::new();
        for (c, r) in &v.vis {
            let all = v.values(r)?;
            let vals: Vec<V> = ids.iter().map(|&i| all[i as usize].clone()).collect();
            out.push(stats(c, &vals));
        }
        Ok(Summary { rows: v.n, columns: out })
    }

    fn materialize(&self, p: &Pipeline) -> Result<DatasetInfo> {
        let v = self.view(p)?;
        let ids = v.ids();
        let mut data: Vec<Vec<V>> = vec![Vec::with_capacity(ids.len()); v.vis.len()];
        for chunk in ids.chunks(20_000) {
            for row in v.cells(chunk)? {
                for (k, x) in row.into_iter().enumerate() {
                    data[k].push(x);
                }
            }
        }
        let n = ids.len();
        let h = self.add(Base::Mem(MemTable { cols: v.columns(), data: data.into_iter().map(Arc::new).collect(), n }));
        let nv = View::of(self.base(&h)?);
        Ok(self.info(&h, p.describe(), &nv))
    }

    fn export_csv(&self, req: &ExportRequest) -> Result<ExportInfo> {
        let v = self.view(&req.pipeline)?;
        let f = std::fs::File::create(&req.path).map_err(|e| io(e, &req.path))?;
        let mut w = std::io::BufWriter::new(f);
        let names: Vec<String> = v.vis.iter().map(|c| c.0.name.clone()).collect();
        csv::write_record(&mut w, &names, ',').map_err(|e| io(e, &req.path))?;
        let ids = v.ids();
        for chunk in ids.chunks(20_000) {
            for row in v.cells(chunk)? {
                let cells: Vec<String> = row.iter().map(json_text).collect();
                csv::write_record(&mut w, &cells, ',').map_err(|e| io(e, &req.path))?;
            }
        }
        w.flush().map_err(|e| io(e, &req.path))?;
        drop(w);
        let bytes = std::fs::metadata(&req.path).map(|m| m.len()).unwrap_or(0);
        Ok(ExportInfo { path: req.path.clone(), rows: v.n, bytes })
    }
}
