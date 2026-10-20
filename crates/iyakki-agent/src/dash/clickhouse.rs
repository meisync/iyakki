//! ClickHouse over its HTTP interface (port 8123): the SQL goes in the POST body, `default_format=JSON` brings back typed
//! columns. Every request carries `readonly=2` (no writes, settings may be tuned except `readonly` itself), a query timeout and
//! `max_result_rows` / `max_result_bytes` with `result_overflow_mode=break`, so a big table can never flood the screen.
//! Use a user that is read-only on the server as well: that is the real guard, ours is a second one.

use serde_json::Value;

use super::connector::*;
use super::frame::*;
use super::readonly::{sql_read_only, Dialect};

pub struct ClickHouse {
    id: String,
    http: Http,
    database: String,
}

pub fn info() -> KindInfo {
    let mut fields = net_fields("http://localhost:8123");
    fields.insert(3, Field::new("database", "Database", FieldKind::Text, "default database (optional)").default("default"));
    KindInfo {
        kind: "clickhouse".into(),
        label: "ClickHouse".into(),
        language: Lang::Sql,
        about: "SQL on ClickHouse through its HTTP port (8123 or 8443). Create a read-only user for dashboards.".into(),
        fields,
        example: "http://localhost:8123".into(),
    }
}

impl ClickHouse {
    pub fn new(cfg: &SourceConfig, d: &Deps) -> Result<ClickHouse, DashError> {
        Ok(ClickHouse { id: cfg.id.clone(), http: Http::from_config(cfg, d, &format!("ClickHouse “{}”", cfg.name))?, database: cfg.value("database").to_string() })
    }

    pub fn with_http(id: &str, http: Http) -> ClickHouse {
        ClickHouse { id: id.into(), http, database: String::new() }
    }

    /// Run `sql` with named parameters (`{db:String}` in the text) and return the parsed JSON answer.
    fn run(&self, sql: &str, params: &[(&str, &str)], limit: usize, timeout_s: u64, cancel: &Cancel) -> Result<Value, DashError> {
        sql_read_only(sql, Dialect::ClickHouse).map_err(DashError::read_only)?;
        let mut path = format!(
            "/?default_format=JSON&readonly=2&max_execution_time={timeout_s}&max_result_rows={limit}&max_result_bytes={}&result_overflow_mode=break&output_format_json_quote_64bit_integers=0&output_format_json_quote_denormals=0&wait_end_of_query=0",
            MAX_RESPONSE_BYTES / 2
        );
        if !self.database.is_empty() {
            path.push_str(&format!("&database={}", enc(&self.database)));
        }
        for (k, v) in params {
            path.push_str(&format!("&param_{k}={}", enc(v)));
        }
        let r = self.http.send("POST", &path, &[("Content-Type", "text/plain; charset=utf-8")], sql.as_bytes(), cancel)?;
        let v = r.json().map_err(|_| {
            let t = r.text();
            // an `exception` may arrive as plain text after the status line, or the user chose another FORMAT
            if t.contains("DB::Exception") { DashError::new(ErrKind::BadQuery, format!("ClickHouse: {}", server_message(t.as_bytes()))) } else { DashError::parse("the answer is not JSON").fix("Remove any FORMAT clause from the query: dashboards ask for JSON themselves.") }
        })?;
        if let Some(e) = v.get("exception").and_then(Value::as_str) {
            return Err(DashError::new(ErrKind::BadQuery, format!("ClickHouse: {}", e.trim())));
        }
        Ok(v)
    }
}

fn col_type(t: &str) -> ColType {
    let base = t.trim_start_matches("Nullable(").trim_start_matches("LowCardinality(").trim_start_matches("Nullable(");
    if base.starts_with("DateTime") || base.starts_with("Date") {
        ColType::Time
    } else if base.starts_with("UInt") || base.starts_with("Int") {
        ColType::Int
    } else if base.starts_with("Float") || base.starts_with("Decimal") {
        ColType::Float
    } else if base.starts_with("Bool") {
        ColType::Bool
    } else {
        ColType::Str
    }
}

/// Turn ClickHouse's `FORMAT JSON` object (`meta`, `data` as objects, `rows`, `statistics`) into a frame.
pub fn parse_json(v: &Value, limit: usize) -> Result<Frame, DashError> {
    let meta = v.get("meta").and_then(Value::as_array).ok_or_else(|| DashError::parse("the answer has no `meta` (column list)"))?;
    let mut f = Frame::default();
    for m in meta {
        let name = m.get("name").and_then(Value::as_str).unwrap_or("?").to_string();
        let ty = m.get("type").and_then(Value::as_str).map_or(ColType::Str, col_type);
        f.columns.push(Column { name, ty });
    }
    for r in v.get("data").and_then(Value::as_array).into_iter().flatten() {
        let Some(o) = r.as_object() else { continue };
        f.rows.push(f.columns.iter().map(|c| o.get(&c.name).cloned().unwrap_or(Value::Null)).collect());
    }
    // nested values (arrays, maps, tuples) are shown as text
    for r in &mut f.rows {
        for c in r.iter_mut() {
            if c.is_array() || c.is_object() {
                *c = Value::from(c.to_string());
            }
        }
    }
    let total = v.get("rows_before_limit_at_least").and_then(Value::as_u64);
    if let Some(t) = total.filter(|t| *t as usize > f.rows.len()) {
        f.meta.truncated = true;
        f.meta.notices.push(format!("{} of at least {} rows", f.rows.len(), t));
    }
    if f.rows.len() >= limit {
        f.meta.truncated = true;
        f.meta.notices.push(format!("the result was cut at {limit} rows (max_result_rows)"));
    }
    f.cap(limit, MAX_SERIES, MAX_POINTS);
    f.derive_series();
    Ok(f)
}

impl DataSource for ClickHouse {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "clickhouse"
    }
    fn language(&self) -> Lang {
        Lang::Sql
    }

    fn test(&self, cancel: &Cancel) -> Result<TestInfo, DashError> {
        let v = self.run("SELECT version() AS v, currentUser() AS u", &[], 1, 10, cancel)?;
        let row = v.pointer("/data/0");
        let ver = row.and_then(|r| r.get("v")).and_then(Value::as_str).unwrap_or("?");
        let user = row.and_then(|r| r.get("u")).and_then(Value::as_str).unwrap_or("");
        Ok(TestInfo { ok: true, detail: format!("ClickHouse {ver}{}", if user.is_empty() { String::new() } else { format!(" as {user}") }) })
    }

    fn browse(&self, path: &[String], cancel: &Cancel) -> Result<Vec<Node>, DashError> {
        match path {
            [] => {
                let f = parse_json(&self.run("SELECT name FROM system.databases ORDER BY name", &[], MAX_ROWS, 15, cancel)?, MAX_ROWS)?;
                Ok((0..f.rows.len()).map(|i| Node { name: f.cell(i, 0), kind: NodeKind::Database, detail: String::new(), has_children: true }).collect())
            }
            [db] => {
                let f = parse_json(&self.run("SELECT name, engine, total_rows FROM system.tables WHERE database = {db:String} ORDER BY name", &[("db", db)], MAX_ROWS, 15, cancel)?, MAX_ROWS)?;
                Ok((0..f.rows.len()).map(|i| Node { name: f.cell(i, 0), kind: NodeKind::Table, detail: format!("{}, {} rows", f.cell(i, 1), f.cell(i, 2)), has_children: true }).collect())
            }
            [db, table] => {
                let f = parse_json(&self.run("SELECT name, type FROM system.columns WHERE database = {db:String} AND table = {t:String} ORDER BY position", &[("db", db), ("t", table)], MAX_ROWS, 15, cancel)?, MAX_ROWS)?;
                Ok((0..f.rows.len()).map(|i| Node { name: f.cell(i, 0), kind: NodeKind::Column, detail: f.cell(i, 1), has_children: false }).collect())
            }
            _ => Ok(vec![]),
        }
    }

    fn query(&self, req: &QueryRequest, cancel: &Cancel) -> Result<Frame, DashError> {
        let started = std::time::Instant::now();
        let sql = req.text.trim().trim_end_matches(';').trim();
        if sql.is_empty() {
            return Err(DashError::new(ErrKind::BadQuery, "the SQL is empty").fix("Write a SELECT, for example SELECT * FROM system.tables LIMIT 10."));
        }
        let v = self.run(sql, &[], req.limit(), req.timeout_s.unwrap_or(DEFAULT_TIMEOUT_S).clamp(1, MAX_TIMEOUT_S), cancel)?;
        let mut f = parse_json(&v, req.limit())?;
        f.meta.took_ms = v.pointer("/statistics/elapsed").and_then(Value::as_f64).map_or_else(|| took(started), |s| (s * 1000.0) as u64);
        f.meta.executed = sql.to_string();
        Ok(f)
    }

    fn starter(&self, target: &str) -> String {
        match target.split_once('.') {
            Some((db, t)) if is_ident(db) && is_ident(t) => format!("SELECT * FROM {db}.{t} LIMIT 100"),
            _ if is_ident(target) => format!("SELECT * FROM {target} LIMIT 100"),
            _ => "SELECT * FROM system.tables LIMIT 100".into(),
        }
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::connector::fake::*;
    use crate::wsexec::FakeRunner;
    use std::sync::Arc;

    fn src(up: &Arc<FakeUp>) -> ClickHouse {
        let d = deps(up, &[], Arc::new(FakeRunner::new(&[])));
        ClickHouse::new(&cfg("clickhouse", "http://ch:8123"), &d).unwrap()
    }

    const TOP: &str = r#"{"meta":[{"name":"query_kind","type":"Enum8('Select' = 1)"},{"name":"normalized","type":"String"},{"name":"calls","type":"UInt64"},{"name":"avg_ms","type":"Float64"},{"name":"t","type":"DateTime"},{"name":"tags","type":"Array(String)"}],
        "data":[{"query_kind":"Select","normalized":"SELECT ? FROM t","calls":120,"avg_ms":12.5,"t":"2026-10-15 12:00:00","tags":["a"]},
                {"query_kind":"Select","normalized":"SELECT ?","calls":3,"avg_ms":0.25,"t":"2026-10-15 12:01:00","tags":[]}],
        "rows":2,"rows_before_limit_at_least":50,"statistics":{"elapsed":0.0123,"rows_read":100,"bytes_read":1000}}"#;

    #[test]
    fn json_format_becomes_typed_columns_and_rows() {
        let up = FakeUp::new().on("POST", 200, TOP);
        let mut r = QueryRequest::new("SELECT * FROM system.query_log", range());
        r.limit = 100;
        let f = src(&up).query(&r, &Cancel::new()).unwrap();
        assert_eq!(f.columns.iter().map(|c| c.ty).collect::<Vec<_>>(), vec![ColType::Str, ColType::Str, ColType::Int, ColType::Float, ColType::Time, ColType::Str]);
        assert_eq!(f.cell(0, 2), "120");
        assert_eq!(f.cell(0, 5), "[\"a\"]");
        assert_eq!(f.meta.took_ms, 12);
        assert!(f.meta.truncated && f.meta.notices[0].contains("2 of at least 50"), "{:?}", f.meta);
        let url = up.urls().remove(0);
        for want in ["default_format=JSON", "readonly=2", "max_execution_time=30", "max_result_rows=100", "result_overflow_mode=break", "quote_64bit_integers=0"] {
            assert!(url.contains(want), "{want} in {url}");
        }
        assert!(up.bodies()[0].starts_with("SELECT * FROM system.query_log"));
    }

    #[test]
    fn writes_never_leave_the_machine() {
        let up = FakeUp::new();
        let s = src(&up);
        for q in ["INSERT INTO t VALUES (1)", "SELECT 1; DROP TABLE t", "DROP TABLE x", "SELECT * FROM file('/etc/passwd')", "ALTER TABLE t DELETE WHERE 1", "OPTIMIZE TABLE t"] {
            let e = s.query(&QueryRequest::new(q, range()), &Cancel::new()).unwrap_err();
            assert_eq!(e.kind, ErrKind::ReadOnly, "{q}");
        }
        assert!(up.urls().is_empty());
    }

    #[test]
    fn server_errors_keep_their_message() {
        let up = FakeUp::new().on("POST", 404, "Code: 60. DB::Exception: Table default.nope does not exist. (UNKNOWN_TABLE) (version 24.3.1.1)\n");
        let e = src(&up).query(&QueryRequest::new("SELECT * FROM nope", range()), &Cancel::new()).unwrap_err();
        assert!(e.message.contains("Table default.nope does not exist"), "{e}");
        let up = FakeUp::new().on("POST", 500, "Code: 159. DB::Exception: Timeout exceeded: elapsed 30 seconds. (TIMEOUT_EXCEEDED)");
        assert!(src(&up).query(&QueryRequest::new("SELECT sleep(3)", range()), &Cancel::new()).unwrap_err().message.contains("Timeout exceeded"));
        let up = FakeUp::new().on("POST", 200, "1\t2\n");
        let e = src(&up).query(&QueryRequest::new("SELECT 1, 2 FORMAT TSV", range()), &Cancel::new()).unwrap_err();
        assert_eq!(e.kind, ErrKind::Parse);
        assert!(e.to_string().contains("FORMAT"));
        let up = FakeUp::new().on("POST", 200, r#"{"exception":"Code: 241. DB::Exception: Memory limit exceeded"}"#);
        assert!(src(&up).query(&QueryRequest::new("SELECT 1", range()), &Cancel::new()).unwrap_err().message.contains("Memory limit"));
    }

    #[test]
    fn a_time_column_gives_series() {
        let body = r#"{"meta":[{"name":"t","type":"DateTime"},{"name":"host","type":"String"},{"name":"cpu","type":"Float64"}],"data":[
            {"t":"2026-10-15 12:00:00","host":"a","cpu":1.5},{"t":"2026-10-15 12:01:00","host":"a","cpu":2.5},{"t":"2026-10-15 12:00:00","host":"b","cpu":9}],"rows":3}"#;
        let up = FakeUp::new().on("POST", 200, body);
        let f = src(&up).query(&QueryRequest::new("SELECT t, host, cpu FROM m", range()), &Cancel::new()).unwrap();
        assert_eq!(f.series.len(), 2);
        assert_eq!(f.series[0].points.len(), 2);
    }

    #[test]
    fn browse_uses_parameters_not_string_building() {
        let dbs = r#"{"meta":[{"name":"name","type":"String"}],"data":[{"name":"default"},{"name":"system"}]}"#;
        let cols = r#"{"meta":[{"name":"name","type":"String"},{"name":"type","type":"UInt64"}],"data":[{"name":"id","type":"UInt64"}]}"#;
        let up = FakeUp::new().on("system.databases", 200, dbs).on("system.columns", 200, cols);
        let s = src(&up);
        assert_eq!(s.browse(&[], &Cancel::new()).unwrap().len(), 2);
        let c = s.browse(&["we'ird".into(), "t; DROP".into()], &Cancel::new()).unwrap();
        assert_eq!(c[0].name, "id");
        let url = up.urls()[1].clone();
        assert!(url.contains("param_db=we%27ird") && url.contains("param_t=t%3B%20DROP"), "{url}");
        assert!(!up.bodies()[1].contains("we'ird"), "values travel as parameters");
    }

    #[test]
    fn test_reports_version_and_user() {
        let up = FakeUp::new().on("POST", 200, r#"{"meta":[],"data":[{"v":"24.3.1.1","u":"dash_ro"}]}"#);
        assert_eq!(src(&up).test(&Cancel::new()).unwrap().detail, "ClickHouse 24.3.1.1 as dash_ro");
    }

    #[test]
    fn odd_json_never_panics() {
        for body in ["", "null", "[]", "{}", r#"{"meta":5}"#, r#"{"meta":[{"name":1}],"data":[1,null,{"x":[1]}]}"#, r#"{"meta":[{"name":"a","type":"Nullable(Date)"}],"data":[{"a":{"b":1}}]}"#] {
            let up = FakeUp::new().on("POST", 200, body);
            let _ = src(&up).query(&QueryRequest::new("SELECT 1", range()), &Cancel::new());
        }
    }

    #[test]
    fn starters() {
        let s = src(&FakeUp::new());
        assert_eq!(s.starter("default.events"), "SELECT * FROM default.events LIMIT 100");
        assert_eq!(s.starter("x; DROP"), "SELECT * FROM system.tables LIMIT 100");
    }
}
