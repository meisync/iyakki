//! DuckDB through its command-line program (`duckdb`), run by the same argument-array runner as the other tools: no linking,
//! no 40-minute C++ build, nothing in the process. It reads a database file (opened `-readonly`) or CSV / Parquet / JSON
//! files named in the SQL, and prints the result as JSON (`-json`). Reasons and the alternative are in `docs/DASHBOARD.md`.
//!
//! The program is installed from the CLI tile (catalog id `duckdb`), never silently: when it is missing the error says so.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::connector::*;
use super::frame::*;
use super::readonly::{duckdb_paths_ok, sql_read_only, Dialect};
use crate::wsexec::Runner;

pub struct DuckDb {
    id: String,
    program: String,
    runner: std::sync::Arc<dyn Runner>,
    database: Option<PathBuf>,
    files: Vec<PathBuf>,
    path_ok: std::sync::Arc<dyn Fn(&Path) -> bool + Send + Sync>,
    work_dir: PathBuf,
}

pub fn info() -> KindInfo {
    KindInfo {
        kind: "duckdb".into(),
        label: "DuckDB".into(),
        language: Lang::Sql,
        about: "SQL on a DuckDB database file or on CSV / Parquet / JSON files, run by the duckdb command (install it from the CLI tile).".into(),
        fields: vec![
            Field::new("database", "Database file", FieldKind::Path, "a .duckdb / .db file, opened read-only (leave empty to query files only)"),
            Field::new("files", "Data files", FieldKind::Text, "CSV / Parquet / JSON files to explore, separated by commas (full paths)"),
            Field::new("program", "Program", FieldKind::Text, "name or path of the duckdb command").default("duckdb"),
            Field::new("timeout_s", "Timeout (s)", FieldKind::Number, "per query, up to 120").default("30"),
        ],
        example: "/home/you/data/sales.parquet".into(),
    }
}

pub const MISSING: &str = "DuckDB is not installed";
pub const MISSING_FIX: &str = "Open the CLI tile, choose DuckDB and install it (IYAKKI never installs a program without you asking). Then test the source again.";

impl DuckDb {
    pub fn new(cfg: &SourceConfig, d: &Deps) -> Result<DuckDb, DashError> {
        let abs = |p: &str, what: &str| -> Result<PathBuf, DashError> {
            let pb = PathBuf::from(p.trim());
            if !pb.is_absolute() || p.contains("..") {
                return Err(DashError::config(format!("{what} `{p}` must be a full path (no `..`)")));
            }
            if !(d.path_ok)(&pb) {
                return Err(DashError::config(format!("you may not read `{p}`")).fix("Pick a file inside your home folder, a workspace or a shared host folder."));
            }
            Ok(pb)
        };
        let database = match cfg.value("database") {
            "" => None,
            p => Some(abs(p, "the database file")?),
        };
        let files = cfg.value("files").split(',').map(str::trim).filter(|s| !s.is_empty()).map(|p| abs(p, "the data file")).collect::<Result<Vec<_>, _>>()?;
        if database.is_none() && files.is_empty() {
            return Err(DashError::config("give a database file or at least one data file"));
        }
        let program = match cfg.value("program") {
            "" => "duckdb".to_string(),
            p if p.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c)) && !p.contains("..") => p.to_string(),
            _ => return Err(DashError::config("the program name has odd characters")),
        };
        Ok(DuckDb { id: cfg.id.clone(), program, runner: d.runner.clone(), database, files, path_ok: d.path_ok.clone(), work_dir: d.work_dir.clone() })
    }

    /// Run one SQL text and return the rows as a frame.
    fn run(&self, sql: &str, limit: usize, timeout_s: u64, cancel: &Cancel) -> Result<Frame, DashError> {
        cancel.check()?;
        sql_read_only(sql, Dialect::DuckDb).map_err(DashError::read_only)?;
        duckdb_paths_ok(sql, &*self.path_ok).map_err(DashError::read_only)?;
        let text = sql.trim().trim_end_matches(';').trim();
        let first = text.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        // wrap so the program itself stops at the row limit (statements like DESCRIBE or SHOW are left alone)
        let sql = if matches!(first.as_str(), "select" | "with" | "from" | "values") { format!("SELECT * FROM ({text}) AS q LIMIT {}", limit + 1) } else { text.to_string() };
        let mut argv = vec![self.program.clone(), "-json".into(), "-batch".into(), "-no-stdin".into(), "-init".into(), "/dev/null".into()];
        if let Some(db) = &self.database {
            argv.push("-readonly".into());
            argv.push(db.to_string_lossy().into_owned());
        }
        argv.push("-c".into());
        argv.push(sql);
        let started = std::time::Instant::now();
        let out = self.runner.run(&self.work_dir, &argv, &[], Duration::from_secs(timeout_s), MAX_RESPONSE_BYTES, None);
        cancel.check()?;
        if out.missing {
            return Err(DashError::new(ErrKind::Missing, MISSING).fix(MISSING_FIX));
        }
        if out.timed_out {
            return Err(DashError::new(ErrKind::Limit, format!("DuckDB stopped after {timeout_s} s")).fix("Use a simpler query or raise the timeout in the source (up to 120 s)."));
        }
        if !out.ok() {
            let msg = out.stderr.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).or_else(|| out.stdout.lines().next().map(String::from)).unwrap_or_else(|| "DuckDB failed".into());
            return Err(DashError::new(ErrKind::BadQuery, format!("DuckDB: {}", msg.trim_start_matches("Error: "))));
        }
        if out.truncated {
            return Err(DashError::new(ErrKind::Limit, "the result is larger than the allowed size").fix("Add a LIMIT, or select fewer columns."));
        }
        let mut f = parse_json_output(&out.stdout, limit)?;
        f.meta.took_ms = took(started);
        f.meta.executed = text.to_string();
        Ok(f)
    }

    fn file_func(path: &Path) -> &'static str {
        match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("parquet") => "read_parquet",
            Some("json" | "jsonl" | "ndjson") => "read_json_auto",
            _ => "read_csv_auto",
        }
    }

    /// `SELECT * FROM read_csv_auto('/path')` for a data file.
    pub fn select_file(path: &Path) -> String {
        format!("SELECT * FROM {}('{}')", Self::file_func(path), path.to_string_lossy().replace('\'', "''"))
    }
}

/// DuckDB's `-json` prints `[{"a":1},...]` (nothing at all for an empty result of some statements).
pub fn parse_json_output(stdout: &str, limit: usize) -> Result<Frame, DashError> {
    let t = stdout.trim();
    let mut f = Frame::default();
    if t.is_empty() || t == "[]" {
        return Ok(f);
    }
    let v: Value = serde_json::from_str(t).map_err(|_| DashError::parse("DuckDB's output was not JSON").fix("Use a duckdb version that has -json (0.8 or newer)."))?;
    let rows = v.as_array().ok_or_else(|| DashError::parse("DuckDB's output is not a list of rows"))?;
    // serde_json sorts keys; the column order is the order of the first row as printed
    let mut names: Vec<String> = first_row_keys(t);
    for r in rows {
        if let Some(o) = r.as_object() {
            for k in o.keys() {
                if !names.contains(k) {
                    names.push(k.clone());
                }
            }
        }
    }
    f.rows = rows.iter().filter_map(Value::as_object).map(|o| names.iter().map(|n| o.get(n).cloned().unwrap_or(Value::Null)).collect()).collect();
    for r in &mut f.rows {
        for c in r.iter_mut() {
            if c.is_array() || c.is_object() {
                *c = Value::from(c.to_string());
            }
        }
    }
    f.columns = names.iter().enumerate().map(|(i, n)| Column { name: n.clone(), ty: infer_type(n, f.rows.iter().map(|r| &r[i])) }).collect();
    if f.rows.len() > limit {
        f.rows.truncate(limit);
        f.meta.truncated = true;
        f.meta.notices.push(format!("the result was cut at {limit} rows"));
    }
    f.derive_series();
    Ok(f)
}

/// The keys of the first `{...}` in `text`, in the order they appear (strings and nesting respected).
pub fn first_row_keys(text: &str) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    let (mut i, mut depth, mut keys) = (0, 0i32, vec![]);
    while i < c.len() {
        match c[i] {
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth <= 1 && !keys.is_empty() {
                    break;
                }
            }
            '"' => {
                let mut s = String::new();
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if c[i] == '\\' && i + 1 < c.len() {
                        i += 1;
                        s.push(match c[i] { 'n' => '\n', 't' => '\t', o => o });
                    } else {
                        s.push(c[i]);
                    }
                    i += 1;
                }
                // a key is a string at depth 2 ([ { "k" ) followed by a colon
                if depth == 2 {
                    let mut j = i + 1;
                    while j < c.len() && c[j].is_whitespace() {
                        j += 1;
                    }
                    if c.get(j) == Some(&':') && !keys.contains(&s) {
                        keys.push(s);
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    keys
}

fn sql_str(s: &str) -> String {
    s.replace('\'', "''")
}

impl DataSource for DuckDb {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "duckdb"
    }
    fn language(&self) -> Lang {
        Lang::Sql
    }

    fn test(&self, cancel: &Cancel) -> Result<TestInfo, DashError> {
        let f = self.run("SELECT version() AS v", 1, 15, cancel)?;
        let mut detail = format!("DuckDB {}", f.cell(0, 0));
        if let Some(db) = &self.database {
            if !db.exists() {
                return Err(DashError::config(format!("the database file {} does not exist", db.display())).fix("Check the path in the source."));
            }
            detail.push_str(&format!(", {}", db.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        }
        for p in &self.files {
            if !p.is_file() {
                return Err(DashError::config(format!("{} is not a file", p.display())).fix("Check the path in the source."));
            }
        }
        if !self.files.is_empty() {
            detail.push_str(&format!(", {} file(s)", self.files.len()));
        }
        Ok(TestInfo { ok: true, detail })
    }

    fn browse(&self, path: &[String], cancel: &Cancel) -> Result<Vec<Node>, DashError> {
        match path {
            [] => {
                let mut out: Vec<Node> = vec![];
                if self.database.is_some() {
                    let f = self.run("SELECT table_name, table_type FROM information_schema.tables WHERE table_schema NOT IN ('information_schema','pg_catalog') ORDER BY table_name", MAX_ROWS, 15, cancel)?;
                    out.extend((0..f.rows.len()).map(|i| Node { name: f.cell(i, 0), kind: NodeKind::Table, detail: f.cell(i, 1).to_lowercase(), has_children: true }));
                }
                out.extend(self.files.iter().map(|p| Node { name: p.to_string_lossy().into_owned(), kind: NodeKind::File, detail: Self::file_func(p).trim_start_matches("read_").trim_end_matches("_auto").to_string(), has_children: true }));
                Ok(out)
            }
            [t] => {
                let f = if self.files.iter().any(|p| p.to_string_lossy() == t.as_str()) {
                    self.run(&format!("DESCRIBE {}", Self::select_file(Path::new(t))), MAX_ROWS, 15, cancel)?
                } else {
                    self.run(&format!("SELECT column_name AS column_name, data_type AS column_type FROM information_schema.columns WHERE table_name = '{}' ORDER BY ordinal_position", sql_str(t)), MAX_ROWS, 15, cancel)?
                };
                let (n, ty) = (f.column_index("column_name").unwrap_or(0), f.column_index("column_type").unwrap_or(1));
                Ok((0..f.rows.len()).map(|i| Node { name: f.cell(i, n), kind: NodeKind::Column, detail: f.cell(i, ty), has_children: false }).collect())
            }
            _ => Ok(vec![]),
        }
    }

    fn query(&self, req: &QueryRequest, cancel: &Cancel) -> Result<Frame, DashError> {
        if req.text.trim().is_empty() {
            return Err(DashError::new(ErrKind::BadQuery, "the SQL is empty").fix("Pick a file or table to start from, or write a SELECT."));
        }
        let mut f = self.run(&req.text, req.limit(), req.timeout_s.unwrap_or(DEFAULT_TIMEOUT_S).clamp(1, MAX_TIMEOUT_S), cancel)?;
        f.cap(req.limit(), MAX_SERIES, MAX_POINTS);
        Ok(f)
    }

    fn starter(&self, target: &str) -> String {
        let p = Path::new(target);
        if p.is_absolute() {
            format!("{} LIMIT 100", Self::select_file(p))
        } else if target.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !target.is_empty() {
            format!("SELECT * FROM {target} LIMIT 100")
        } else {
            "SELECT 1".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::connector::fake::*;
    use crate::wsexec::FakeRunner;
    use std::sync::Arc;

    fn src(r: Arc<FakeRunner>, files: &str, db: &str) -> DuckDb {
        let d = deps(&FakeUp::new(), &[], r);
        let mut c = cfg("duckdb", "");
        c.values.clear();
        if !files.is_empty() {
            c.values.insert("files".into(), files.into());
        }
        if !db.is_empty() {
            c.values.insert("database".into(), db.into());
        }
        DuckDb::new(&c, &d).unwrap()
    }

    #[test]
    fn json_output_becomes_a_frame_and_the_command_line_is_safe() {
        let r = Arc::new(FakeRunner::new(&["duckdb"]));
        r.answer("duckdb", r#"[{"region":"north","units":10,"price":2.5,"day":"2026-10-14"},{"region":"south","units":7,"price":3,"day":"2026-10-15"}]"#);
        let s = src(r.clone(), "/home/amy/sales.csv", "");
        let f = s.query(&QueryRequest::new("SELECT * FROM read_csv_auto('/home/amy/sales.csv')", range()), &Cancel::new()).unwrap();
        assert_eq!(f.rows.len(), 2);
        assert_eq!(f.columns.iter().map(|c| c.ty).collect::<Vec<_>>(), vec![ColType::Str, ColType::Int, ColType::Float, ColType::Time]);
        let call = &r.calls()[0];
        assert_eq!(&call.argv[..4], &["duckdb", "-json", "-batch", "-no-stdin"]);
        assert!(call.argv.last().unwrap().starts_with("SELECT * FROM (SELECT * FROM read_csv_auto("), "{:?}", call.argv);
        assert!(call.argv.last().unwrap().ends_with("LIMIT 1001"));
        assert!(call.env.is_empty());
    }

    #[test]
    fn a_database_is_opened_readonly() {
        let r = Arc::new(FakeRunner::new(&["duckdb"]));
        r.answer("duckdb", r#"[{"v":"v1.1.0"}]"#);
        let s = src(r.clone(), "", "/home/amy/x.duckdb");
        let _ = s.query(&QueryRequest::new("SELECT 1", range()), &Cancel::new()).unwrap();
        let argv = &r.calls()[0].argv;
        let i = argv.iter().position(|a| a == "-readonly").unwrap();
        assert_eq!(argv[i + 1], "/home/amy/x.duckdb");
    }

    #[test]
    fn writes_and_outside_files_never_reach_the_program() {
        let r = Arc::new(FakeRunner::new(&["duckdb"]));
        let s = src(r.clone(), "/home/amy/a.csv", "");
        for q in ["COPY (SELECT 1) TO '/home/amy/x.csv'", "INSTALL httpfs", "ATTACH '/home/amy/x.db'", "SELECT 1; DROP TABLE t", "CREATE TABLE t AS SELECT 1", "SELECT * FROM read_csv_auto('/etc/passwd')", "SELECT * FROM read_csv_auto('https://evil/x.csv')", "SELECT getenv('HOME')", "PRAGMA database_list", "SET enable_external_access=true"] {
            assert!(s.query(&QueryRequest::new(q, range()), &Cancel::new()).is_err(), "{q}");
        }
        assert!(r.calls().is_empty());
    }

    #[test]
    fn missing_program_and_failures_are_explained() {
        let r = Arc::new(FakeRunner::new(&[]));
        let s = src(r, "/home/amy/a.csv", "");
        // the fake runner answers with code 1 for no rule: a SQL error
        let e = s.query(&QueryRequest::new("SELECT 1", range()), &Cancel::new()).unwrap_err();
        assert_eq!(e.kind, ErrKind::BadQuery);
        let r = Arc::new(FakeRunner::new(&["duckdb"]));
        r.fail("duckdb", 1, "Error: Catalog Error: Table with name nope does not exist!\nDid you mean \"t\"?");
        let e = src(r, "/home/amy/a.csv", "").query(&QueryRequest::new("SELECT * FROM nope", range()), &Cancel::new()).unwrap_err();
        assert!(e.message.contains("Catalog Error: Table with name nope does not exist"), "{e}");
        // not installed: the real runner reports `missing`
        let d = deps(&FakeUp::new(), &[], Arc::new(crate::wsexec::System));
        let mut c = cfg("duckdb", "");
        c.values.clear();
        c.values.insert("files".into(), "/home/amy/a.csv".into());
        c.values.insert("program".into(), "duckdb-not-installed-xyz".into());
        let e = DuckDb::new(&c, &d).unwrap().query(&QueryRequest::new("SELECT 1", range()), &Cancel::new()).unwrap_err();
        assert_eq!(e.kind, ErrKind::Missing);
        assert!(e.to_string().contains("CLI tile"), "{e}");
    }

    #[test]
    fn config_rejects_paths_outside_and_relative() {
        let d = deps(&FakeUp::new(), &[], Arc::new(FakeRunner::new(&[])));
        for bad in ["/etc/passwd", "relative.csv", "/home/amy/../bob/x.csv"] {
            let mut c = cfg("duckdb", "");
            c.values.clear();
            c.values.insert("files".into(), bad.into());
            assert!(DuckDb::new(&c, &d).is_err(), "{bad}");
        }
        let mut c = cfg("duckdb", "");
        c.values.clear();
        assert!(DuckDb::new(&c, &d).is_err(), "nothing to query");
    }

    #[test]
    fn browse_and_starters() {
        let r = Arc::new(FakeRunner::new(&["duckdb"]));
        r.answer("DESCRIBE", r#"[{"column_name":"id","column_type":"BIGINT","null":"YES"},{"column_name":"day","column_type":"DATE"}]"#);
        let s = src(r, "/home/amy/a.parquet", "");
        let top = s.browse(&[], &Cancel::new()).unwrap();
        assert_eq!((top[0].name.as_str(), top[0].kind, top[0].detail.as_str()), ("/home/amy/a.parquet", NodeKind::File, "parquet"));
        let cols = s.browse(&["/home/amy/a.parquet".into()], &Cancel::new()).unwrap();
        assert_eq!(cols[1].detail, "DATE");
        assert_eq!(s.starter("/home/amy/it's.csv"), "SELECT * FROM read_csv_auto('/home/amy/it''s.csv') LIMIT 100");
        assert_eq!(s.starter("/home/amy/a.parquet"), "SELECT * FROM read_parquet('/home/amy/a.parquet') LIMIT 100");
    }

    #[test]
    fn odd_output_never_panics() {
        for t in ["", "[]", "null", "{}", "[1,2]", "[{\"a\":[1,{\"b\":2}]}]", "Error: x", "[{\"a\":1},", "[{\"a\":1},{\"b\":2}]"] {
            let _ = parse_json_output(t, 10);
        }
        let f = parse_json_output("[{\"a\":1},{\"b\":2}]", 10).unwrap();
        assert_eq!(f.columns.len(), 2);
        let f = parse_json_output("[{\"a\":1},{\"a\":2},{\"a\":3}]", 2).unwrap();
        assert!(f.meta.truncated && f.rows.len() == 2);
    }
}
