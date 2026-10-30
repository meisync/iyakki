//! The read-only rule. Dashboards only read: a SQL text must be exactly one SELECT-like statement, an Elasticsearch call
//! must be a search, count, mapping or cat call, a Prometheus call must be a query call. The checks run before anything
//! is sent, and the servers are asked to enforce the same where they can (ClickHouse `readonly=2`, DuckDB `-readonly`).
//! The SQL check is deliberately simple and strict: when in doubt it refuses.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Generic,
    ClickHouse,
    DuckDb,
}

/// Words that start or hide a change, a load of code, a file write, or a setting.
const FORBIDDEN: &[&str] = &[
    "insert", "update", "delete", "drop", "alter", "create", "truncate", "attach", "detach", "grant", "revoke", "optimize", "rename", "kill", "merge", "upsert", "pragma", "install", "into", "outfile", "set", "system", "undrop", "exchange",
    "replace",
];
/// Words that are fine as functions (`replace(a, b, c)`, `truncate(x)`, `left`) and forbidden otherwise.
const FUNCTION_OK: &[&str] = &["replace", "truncate", "set", "system"];
/// ClickHouse table functions that reach files, the network or other servers.
const CH_FUNCS: &[&str] = &["file", "url", "s3", "s3cluster", "hdfs", "remote", "remotesecure", "cluster", "clusterallreplicas", "executable", "jdbc", "odbc", "mysql", "postgresql", "mongodb", "redis", "sqlite", "azureblobstorage", "gcs", "input", "deltalake", "iceberg", "hudi", "urlcluster", "filecluster", "mergetree", "dictionary"];
/// DuckDB functions and settings that leave the sandbox of "read these files".
const DUCK_FUNCS: &[&str] = &["read_text", "read_blob", "glob_all", "sniff_csv_all", "getenv", "current_setting", "duckdb_secrets", "which_secret", "enable_external_access", "shell", "system"];

#[derive(Debug, PartialEq, Eq)]
struct Tok {
    word: String,
    /// The next significant character is `(`.
    call: bool,
}

/// Remove comments, keep string and quoted-identifier contents out of the token list. Returns the statements' tokens (split at `;`),
/// the quoted string literals, and an error for an unterminated quote or comment.
fn scan(sql: &str, dialect: Dialect) -> Result<(Vec<Vec<Tok>>, Vec<String>), String> {
    let c: Vec<char> = sql.chars().collect();
    let (mut i, mut stmts, mut cur, mut lits) = (0, vec![], Vec::<Tok>::new(), vec![]);
    let mut word = String::new();
    let flush = |word: &mut String, cur: &mut Vec<Tok>, next_is_paren: bool| {
        if !word.is_empty() {
            cur.push(Tok { word: word.to_ascii_lowercase(), call: next_is_paren });
            word.clear();
        }
    };
    let next_sig = |from: usize| c[from..].iter().find(|ch| !ch.is_whitespace()).copied();
    while i < c.len() {
        let ch = c[i];
        match ch {
            '-' if c.get(i + 1) == Some(&'-') => {
                flush(&mut word, &mut cur, false);
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '#' if dialect == Dialect::ClickHouse => {
                flush(&mut word, &mut cur, false);
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if c.get(i + 1) == Some(&'*') => {
                flush(&mut word, &mut cur, false);
                i += 2;
                let mut depth = 1;
                while i < c.len() && depth > 0 {
                    if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if depth > 0 {
                    return Err("a /* comment is never closed".into());
                }
                continue;
            }
            '\'' | '"' | '`' => {
                flush(&mut word, &mut cur, false);
                let q = ch;
                let mut lit = String::new();
                i += 1;
                let mut closed = false;
                while i < c.len() {
                    if c[i] == '\\' && dialect == Dialect::ClickHouse && i + 1 < c.len() {
                        lit.push(c[i + 1]);
                        i += 2;
                    } else if c[i] == q {
                        if c.get(i + 1) == Some(&q) {
                            lit.push(q);
                            i += 2;
                        } else {
                            closed = true;
                            i += 1;
                            break;
                        }
                    } else {
                        lit.push(c[i]);
                        i += 1;
                    }
                }
                if !closed {
                    return Err(format!("a {q} quote is never closed"));
                }
                if q == '\'' {
                    lits.push(lit);
                } else {
                    // a quoted name stands for a name: keep it as an opaque token so `"update"` as a column is fine
                    cur.push(Tok { word: format!("\u{0}{lit}"), call: next_sig(i) == Some('(') });
                }
                continue;
            }
            ';' => {
                flush(&mut word, &mut cur, false);
                stmts.push(std::mem::take(&mut cur));
            }
            ch if ch.is_alphanumeric() || ch == '_' || ch == '$' => word.push(ch),
            _ => {
                let nxt = next_sig(i);
                flush(&mut word, &mut cur, ch.is_whitespace() && nxt == Some('(') || ch == '(');
                if ch == '.' {
                    if let Some(l) = cur.last_mut() {
                        l.call = false;
                    }
                }
            }
        }
        i += 1;
    }
    flush(&mut word, &mut cur, false);
    stmts.push(cur);
    Ok((stmts, lits))
}

/// `Ok` when `sql` is a single read-only statement for this dialect; otherwise a sentence that says what to change.
pub fn sql_read_only(sql: &str, dialect: Dialect) -> Result<(), String> {
    let (mut stmts, _) = scan(sql, dialect)?;
    stmts.retain(|s| !s.is_empty());
    match stmts.len() {
        0 => return Err("the query is empty".into()),
        1 => {}
        _ => return Err("only one statement can be run at a time (a second statement after `;` is refused)".into()),
    }
    let toks = &stmts[0];
    let first = toks[0].word.as_str();
    let starts = ["select", "with", "show", "describe", "desc", "explain", "values", "from", "table"];
    if !starts.contains(&first) {
        return Err(format!("only read queries are allowed (SELECT, WITH, SHOW, DESCRIBE, EXPLAIN); this one starts with `{first}`"));
    }
    for (i, t) in toks.iter().enumerate() {
        let w = t.word.as_str();
        if w.starts_with('\u{0}') {
            continue;
        }
        if FORBIDDEN.contains(&w) && !(FUNCTION_OK.contains(&w) && t.call) {
            // `system.tables` is a database name: the dot follows
            if w == "system" && toks.get(i + 1).is_some() && !t.call && dialect == Dialect::ClickHouse {
                continue;
            }
            return Err(format!("`{w}` is not allowed: dashboards only read data"));
        }
        if t.call {
            if dialect == Dialect::ClickHouse && CH_FUNCS.contains(&w) {
                return Err(format!("the table function `{w}(...)` reads files or other servers and is not allowed"));
            }
            if dialect == Dialect::DuckDb && DUCK_FUNCS.contains(&w) {
                return Err(format!("`{w}(...)` is not allowed"));
            }
        }
        if dialect == Dialect::ClickHouse && w == "readonly" {
            return Err("changing `readonly` is not allowed".into());
        }
        if dialect == Dialect::DuckDb && matches!(w, "httpfs" | "sqlite_scan" | "postgres_scan" | "mysql_scan") {
            return Err(format!("`{w}` reaches other systems and is not allowed"));
        }
    }
    Ok(())
}

/// Words that write, load code or files, change users or the server, or call a procedure (in Cypher). A property named like one
/// (`n.set`) is fine; a quoted name or a string is never looked at.
const CYPHER_FORBIDDEN: &[&str] = &[
    "create", "merge", "delete", "detach", "set", "remove", "drop", "foreach", "load", "call", "grant", "deny", "revoke", "alter", "rename", "start", "stop", "terminate", "dbms", "apoc", "use", "enable", "disable", "reallocate", "dryrun", "csv", "copy", "yield", "profile",
];

/// `Ok` when `text` is one read-only Cypher query: it starts with MATCH, OPTIONAL MATCH, WITH, UNWIND, RETURN, SHOW or EXPLAIN and holds none of
/// [`CYPHER_FORBIDDEN`]. Cypher has no `--` comment (`(a)--(b)` is a relationship), only `//` and `/* */`, so this has its own scanner.
/// The server is asked for a read transaction as well (`accessMode: Read`); this check is the second guard and refuses when in doubt.
pub fn cypher_read_only(text: &str) -> Result<(), String> {
    let c: Vec<char> = text.chars().collect();
    let (mut i, mut words, mut word) = (0usize, Vec::<(String, bool)>::new(), String::new());
    let mut statements = 0usize;
    let mut after_dot = false;
    let mut prev_sig = ' ';
    let mut seen_any = false;
    let flush = |word: &mut String, words: &mut Vec<(String, bool)>, after_dot: bool| {
        if !word.is_empty() {
            words.push((word.to_ascii_lowercase(), after_dot));
            word.clear();
        }
    };
    while i < c.len() {
        let ch = c[i];
        match ch {
            '/' if c.get(i + 1) == Some(&'/') => {
                flush(&mut word, &mut words, after_dot);
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if c.get(i + 1) == Some(&'*') => {
                flush(&mut word, &mut words, after_dot);
                i += 2;
                while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                if i >= c.len() {
                    return Err("a /* comment is never closed".into());
                }
                i += 2;
                continue;
            }
            '\'' | '"' | '`' => {
                flush(&mut word, &mut words, after_dot);
                let q = ch;
                i += 1;
                let mut closed = false;
                while i < c.len() {
                    if c[i] == '\\' && q != '`' && i + 1 < c.len() {
                        i += 2;
                    } else if c[i] == q {
                        if q == '`' && c.get(i + 1) == Some(&'`') {
                            i += 2;
                        } else {
                            closed = true;
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                if !closed {
                    return Err(format!("a {q} quote is never closed"));
                }
                seen_any = true;
                prev_sig = q;
                after_dot = false;
                continue;
            }
            ';' => {
                flush(&mut word, &mut words, after_dot);
                if seen_any {
                    statements += 1;
                    seen_any = false;
                    if statements > 0 && c[i + 1..].iter().any(|x| !x.is_whitespace() && *x != ';') {
                        return Err("only one query can be run at a time (a second statement after `;` is refused)".into());
                    }
                }
            }
            x if x.is_alphanumeric() || x == '_' || x == '$' => {
                if word.is_empty() {
                    after_dot = prev_sig == '.';
                }
                word.push(x);
                seen_any = true;
            }
            x => {
                flush(&mut word, &mut words, after_dot);
                if !x.is_whitespace() {
                    prev_sig = x;
                }
            }
        }
        if ch.is_alphanumeric() || ch == '_' || ch == '$' {
            prev_sig = 'a';
        }
        i += 1;
    }
    flush(&mut word, &mut words, after_dot);
    let Some((first, _)) = words.first() else { return Err("the query is empty".into()) };
    let starts = ["match", "optional", "with", "unwind", "return", "show", "explain"];
    if !starts.contains(&first.as_str()) {
        return Err(format!("only read queries are allowed (MATCH, OPTIONAL MATCH, WITH, UNWIND, RETURN, SHOW, EXPLAIN); this one starts with `{first}`"));
    }
    for (w, dotted) in &words {
        if CYPHER_FORBIDDEN.contains(&w.as_str()) && !dotted {
            return Err(format!("`{w}` is not allowed: the graph is only read"));
        }
    }
    Ok(())
}

/// Elasticsearch / OpenSearch: only these (method, path) pairs are ever sent.
pub fn es_request_ok(method: &str, path: &str) -> Result<(), String> {
    let m = method.to_ascii_uppercase();
    if m != "GET" && m != "POST" {
        return Err(format!("{m} is not allowed: Elasticsearch is only read (GET and POST to the search calls)"));
    }
    let p = path.split('?').next().unwrap_or("");
    if p.contains("..") || p.contains("//") || p.contains('\\') || p.contains('%') && p.to_ascii_lowercase().contains("%2e") {
        return Err("that path is not valid".into());
    }
    let parts: Vec<&str> = p.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let tail = |names: &[&str]| parts.last().is_some_and(|l| names.contains(l)) && parts.len() <= 2 && parts[..parts.len() - 1].iter().all(|s| !s.starts_with('_'));
    let ok = match parts.as_slice() {
        [] => m == "GET",
        ["_cat", ..] => m == "GET",
        ["_cluster", "health"] | ["_cluster", "health", _] => m == "GET",
        ["_resolve", "index", _] => m == "GET",
        ["_search"] | ["_count"] | ["_field_caps"] | ["_mapping"] => true,
        [_, last] if ["_search", "_count", "_field_caps"].contains(last) => true,
        [_, "_mapping"] => m == "GET",
        _ => false,
    };
    let _ = tail;
    if ok { Ok(()) } else { Err(format!("`{m} /{}` is not allowed: only _search, _count, _mapping, _field_caps, _cat and _cluster/health are", parts.join("/"))) }
}

/// Prometheus: GET (or POST for long queries) to the query calls only.
pub fn prom_request_ok(method: &str, path: &str) -> Result<(), String> {
    let p = path.split('?').next().unwrap_or("");
    if !(method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("POST")) {
        return Err(format!("{method} is not allowed: Prometheus is only read"));
    }
    if p.contains("..") {
        return Err("that path is not valid".into());
    }
    const OK: &[&str] = &["/api/v1/query", "/api/v1/query_range", "/api/v1/series", "/api/v1/labels", "/api/v1/targets", "/api/v1/metadata", "/api/v1/status/buildinfo", "/api/v1/rules", "/api/v1/alerts"];
    if OK.contains(&p) || (p.starts_with("/api/v1/label/") && p.ends_with("/values") && !p[14..].contains("//")) {
        Ok(())
    } else {
        Err(format!("`{p}` is not allowed: only the query, series, label, targets and metadata calls are"))
    }
}

/// DuckDB: the string literals that look like file names must lie inside the folders the account may read.
pub fn duckdb_paths_ok(sql: &str, path_ok: &dyn Fn(&std::path::Path) -> bool) -> Result<(), String> {
    let (_, lits) = scan(sql, Dialect::DuckDb)?;
    for l in lits {
        let t = l.trim();
        let looks_path = t.contains('/') || t.contains('\\') || t.starts_with('~') || [".csv", ".tsv", ".parquet", ".json", ".jsonl", ".ndjson", ".db", ".duckdb", ".gz", ".zst", ".xlsx"].iter().any(|e| t.to_ascii_lowercase().ends_with(e));
        if !looks_path {
            continue;
        }
        if t.contains("://") || t.starts_with("s3:") || t.starts_with("http") {
            return Err(format!("`{}` is a network address: only local files can be read", crate::dash::frame::clip(t, 60)));
        }
        if t.split(['/', '\\']).any(|p| p == "..") {
            return Err("`..` is not allowed in a file path".into());
        }
        let fixed: &str = t.split(['*', '?', '[']).next().unwrap_or(t);
        let base = if fixed.len() < t.len() { fixed.rsplit_once('/').map_or(fixed, |x| x.0) } else { fixed };
        let base = if base.is_empty() { "/" } else { base };
        let p = std::path::Path::new(base);
        if !p.is_absolute() {
            return Err(format!("`{}` is not an absolute path; use the full path of the file", crate::dash::frame::clip(t, 60)));
        }
        if !path_ok(p) {
            return Err(format!("you may not read `{}` (it is outside your folders)", crate::dash::frame::clip(t, 60)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use Dialect::*;

    #[test]
    fn reads_pass() {
        for q in [
            "SELECT 1",
            "select * from t where a = 'drop table x' limit 5;",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "SHOW TABLES",
            "DESCRIBE t",
            "EXPLAIN SELECT 1",
            "SELECT replace(name, 'a', 'b'), truncate(x) FROM t",
            "SELECT \"update\", `delete` FROM t",
            "-- hello\nSELECT 1 /* ; drop table t */",
            "SELECT * FROM system.tables WHERE database = 'default'",
            "FROM read_csv_auto('/home/a/x.csv') SELECT *",
            "SELECT 'it''s; drop' AS s",
        ] {
            let d = if q.contains("system.tables") { ClickHouse } else { Generic };
            assert_eq!(sql_read_only(q, d), Ok(()), "{q}");
        }
    }

    #[test]
    fn writes_and_tricks_are_refused() {
        for q in [
            "INSERT INTO t VALUES (1)",
            "DROP TABLE t",
            "SELECT 1; DROP TABLE t",
            "SELECT 1;\nDELETE FROM t",
            "SELECT 1 /* x */; /* y */ UPDATE t SET a = 1",
            "WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x",
            "SELECT * INTO new_t FROM t",
            "SELECT * FROM t INTO OUTFILE '/tmp/x'",
            "COPY t TO '/tmp/x.csv'",
            "ATTACH 'x.db'",
            "INSTALL httpfs",
            "PRAGMA enable_external_access=true",
            "SET readonly = 0",
            "TRUNCATE TABLE t",
            "ALTER TABLE t DROP COLUMN a",
            "OPTIMIZE TABLE t",
            "KILL QUERY WHERE 1",
            "SYSTEM RELOAD CONFIG",
            "",
            "  ;  ",
            "SELECT 'unterminated",
            "SELECT 1 /* never closed",
            "-- only a comment",
            "SELECT 1 -- hi\n; DROP TABLE t",
            "CREATE TABLE t (a Int)",
            "UPDATE\tt SET a=1",
            "SeLeCt 1; dRoP table t",
        ] {
            assert!(sql_read_only(q, Generic).is_err(), "{q:?} must be refused");
        }
    }

    #[test]
    fn comment_tricks_cannot_hide_a_statement() {
        // a `--` inside a string is not a comment; a `;` inside a comment is not a split
        assert!(sql_read_only("SELECT '--'; DROP TABLE t", Generic).is_err());
        assert!(sql_read_only("SELECT 1 /* ; */", Generic).is_ok());
        assert!(sql_read_only("SELECT 1 # x\n; DROP TABLE t", ClickHouse).is_err());
        assert!(sql_read_only("SELECT 1 /* a /* nested */ still comment */", Generic).is_ok());
        assert!(sql_read_only("/**/DROP/**/TABLE t", Generic).is_err());
    }

    #[test]
    fn dialect_functions_are_refused() {
        assert!(sql_read_only("SELECT * FROM file('/etc/passwd', 'CSV')", ClickHouse).is_err());
        assert!(sql_read_only("SELECT * FROM url('http://x', CSV)", ClickHouse).is_err());
        assert!(sql_read_only("SELECT * FROM numbers(10)", ClickHouse).is_ok());
        assert!(sql_read_only("SELECT 1 SETTINGS readonly = 0", ClickHouse).is_err());
        assert!(sql_read_only("SELECT getenv('HOME')", DuckDb).is_err());
        assert!(sql_read_only("SELECT * FROM read_text('/etc/passwd')", DuckDb).is_err());
    }

    #[test]
    fn es_calls() {
        for (m, p) in [("POST", "/logs-*/_search"), ("GET", "/_cat/indices"), ("GET", "/idx/_mapping"), ("POST", "/_search"), ("POST", "/i/_count"), ("GET", "/"), ("GET", "/_cluster/health"), ("POST", "/i/_field_caps")] {
            assert_eq!(es_request_ok(m, p), Ok(()), "{m} {p}");
        }
        for (m, p) in [("POST", "/_bulk"), ("DELETE", "/idx"), ("PUT", "/idx"), ("POST", "/idx/_delete_by_query"), ("POST", "/idx/_update/1"), ("GET", "/idx/_doc/1"), ("POST", "/idx/_doc"), ("POST", "/_search/../_bulk"), ("POST", "/_reindex"), ("GET", "/_cat/../x/y/_bulk"), ("POST", "/_security/user/x"), ("POST", "/idx/_forcemerge"), ("POST", "/_all/_close")] {
            assert!(es_request_ok(m, p).is_err(), "{m} {p}");
        }
    }

    #[test]
    fn prom_calls() {
        assert!(prom_request_ok("GET", "/api/v1/query_range").is_ok());
        assert!(prom_request_ok("GET", "/api/v1/label/__name__/values").is_ok());
        assert!(prom_request_ok("POST", "/api/v1/admin/tsdb/delete_series").is_err());
        assert!(prom_request_ok("DELETE", "/api/v1/series").is_err());
        assert!(prom_request_ok("POST", "/-/reload").is_err());
        assert!(prom_request_ok("GET", "/api/v1/../../-/quit").is_err());
    }

    #[test]
    fn duckdb_paths_are_checked() {
        let ok = |p: &std::path::Path| p.starts_with("/home/amy");
        assert!(duckdb_paths_ok("SELECT * FROM read_csv_auto('/home/amy/a.csv')", &ok).is_ok());
        assert!(duckdb_paths_ok("SELECT * FROM read_parquet('/home/amy/d/*.parquet')", &ok).is_ok());
        assert!(duckdb_paths_ok("SELECT * FROM read_csv_auto('/etc/passwd')", &ok).is_err());
        assert!(duckdb_paths_ok("SELECT * FROM read_csv_auto('/home/amy/../bob/x.csv')", &ok).is_err());
        assert!(duckdb_paths_ok("SELECT * FROM read_csv_auto('https://x/y.csv')", &ok).is_err());
        assert!(duckdb_paths_ok("SELECT * FROM read_csv_auto('x.csv')", &ok).is_err());
        assert!(duckdb_paths_ok("SELECT * FROM read_csv_auto('/*/x.csv')", &ok).is_err());
        assert!(duckdb_paths_ok("SELECT 'plain text' AS a", &ok).is_ok());
    }
}
