//! Neo4j (and any server with its HTTP Query API v2) as a data source, for the graph database of Quick install. A query is Cypher.
//! Every request is a read transaction (`"accessMode": "Read"`: the server refuses a write whatever the text says; tested against
//! Neo4j Community 2026.10.1), the text is also checked here ([`super::readonly::cypher_read_only`]) before anything is sent, rows are
//! capped, and nodes, relationships, lists and maps are shown as text. Sign-in is basic, with the password from the vault.

use serde_json::{json, Value};

use super::connector::*;
use super::frame::*;
use super::readonly::cypher_read_only;

pub struct Neo4j {
    id: String,
    http: Http,
    database: String,
}

pub fn info() -> KindInfo {
    let mut fields = net_fields("http://localhost:7474");
    fields.insert(3, Field::new("database", "Database", FieldKind::Text, "the database to query (Neo4j Community has one: neo4j)").default("neo4j"));
    KindInfo {
        kind: "neo4j".into(),
        label: "Neo4j (graph)".into(),
        language: Lang::Cypher,
        about: "Cypher on a Neo4j graph database through its HTTP port (7474, Query API). Reads only: every query is a read transaction. Use basic sign-in.".into(),
        fields,
        example: "http://localhost:7474 (own server) or cluster://iyakki-data/graphdb:7474 (Quick install, runs inside the cluster)".into(),
    }
}

/// A database name goes into the URL: letters, digits, `.`, `-`, `_` only.
pub fn valid_database(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

impl Neo4j {
    pub fn new(cfg: &SourceConfig, d: &Deps) -> Result<Neo4j, DashError> {
        let database = Some(cfg.value("database")).filter(|s| !s.is_empty()).unwrap_or("neo4j").to_string();
        if !valid_database(&database) {
            return Err(DashError::config(format!("Neo4j “{}”: `{}` is not a database name", cfg.name, crate::dash::frame::clip(&database, 40))));
        }
        Ok(Neo4j { id: cfg.id.clone(), http: Http::from_config(cfg, d, &format!("Neo4j “{}”", cfg.name))?, database })
    }

    pub fn with_http(id: &str, http: Http) -> Neo4j {
        Neo4j { id: id.into(), http, database: "neo4j".into() }
    }

    /// Send one statement as a read transaction and return the parsed answer. No text check: the callers that take text from a person
    /// or a model run [`cypher_read_only`] first.
    fn run(&self, statement: &str, cancel: &Cancel) -> Result<Value, DashError> {
        let body = json!({"statement": statement, "accessMode": "Read"}).to_string();
        let path = format!("/db/{}/query/v2", self.database);
        let r = self.http.send("POST", &path, &[("Content-Type", "application/json")], body.as_bytes(), cancel)?;
        let v = r.json()?;
        if let Some(m) = v.pointer("/errors/0/message").and_then(Value::as_str) {
            return Err(DashError::new(ErrKind::BadQuery, format!("Neo4j: {}", m.trim())));
        }
        Ok(v)
    }
}

/// Turn the Query API's `{"data": {"fields": [...], "values": [[...], ...]}}` into a frame, cut at `limit` rows.
pub fn parse_answer(v: &Value, limit: usize) -> Result<Frame, DashError> {
    let fields = v.pointer("/data/fields").and_then(Value::as_array).ok_or_else(|| DashError::parse("the answer has no `data.fields` (column list)"))?;
    let mut f = Frame::default();
    let names: Vec<String> = fields.iter().map(|n| n.as_str().unwrap_or("?").to_string()).collect();
    let mut rows: Vec<Vec<Value>> = vec![];
    for r in v.pointer("/data/values").and_then(Value::as_array).into_iter().flatten() {
        let Some(cells) = r.as_array() else { continue };
        rows.push((0..names.len()).map(|i| match cells.get(i) {
            Some(c) if c.is_array() || c.is_object() => Value::from(c.to_string()),
            Some(c) => c.clone(),
            None => Value::Null,
        }).collect());
    }
    f.columns = names.iter().enumerate().map(|(i, n)| Column { name: n.clone(), ty: infer_type(n, rows.iter().map(|r| &r[i])) }).collect();
    f.rows = rows;
    f.cap(limit, MAX_SERIES, MAX_POINTS);
    if f.meta.truncated {
        f.meta.notices.push(format!("the result was cut at {limit} rows"));
    }
    f.derive_series();
    Ok(f)
}

impl DataSource for Neo4j {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "neo4j"
    }
    fn language(&self) -> Lang {
        Lang::Cypher
    }

    fn test(&self, cancel: &Cancel) -> Result<TestInfo, DashError> {
        let v = self.run("CALL dbms.components() YIELD name, versions, edition RETURN name, versions[0] AS version, edition", cancel)?;
        let f = parse_answer(&v, 1)?;
        let (name, ver, ed) = (f.cell(0, 0), f.cell(0, 1), f.cell(0, 2));
        Ok(TestInfo { ok: true, detail: format!("{} {} {}", if name.is_empty() { "Neo4j" } else { &name }, ver, ed).trim().to_string() })
    }

    fn browse(&self, path: &[String], cancel: &Cancel) -> Result<Vec<Node>, DashError> {
        match path {
            [] => {
                let f = parse_answer(&self.run("CALL db.labels() YIELD label RETURN label ORDER BY label", cancel)?, MAX_ROWS)?;
                let mut out: Vec<Node> = (0..f.rows.len()).map(|i| Node { name: f.cell(i, 0), kind: NodeKind::Label, detail: "node label".into(), has_children: true }).collect();
                let r = parse_answer(&self.run("CALL db.relationshipTypes() YIELD relationshipType RETURN relationshipType ORDER BY relationshipType", cancel)?, MAX_ROWS)?;
                out.extend((0..r.rows.len()).map(|i| Node { name: r.cell(i, 0), kind: NodeKind::Field, detail: "relationship type".into(), has_children: false }));
                Ok(out)
            }
            [label] => {
                if !is_ident(label) {
                    return Err(DashError::new(ErrKind::BadQuery, "that is not a label name"));
                }
                let q = format!("MATCH (n:`{label}`) WITH n LIMIT 200 UNWIND keys(n) AS k RETURN DISTINCT k ORDER BY k");
                let f = parse_answer(&self.run(&q, cancel)?, MAX_ROWS)?;
                Ok((0..f.rows.len()).map(|i| Node { name: f.cell(i, 0), kind: NodeKind::Field, detail: "property".into(), has_children: false }).collect())
            }
            _ => Ok(vec![]),
        }
    }

    fn query(&self, req: &QueryRequest, cancel: &Cancel) -> Result<Frame, DashError> {
        let started = std::time::Instant::now();
        let text = req.text.trim().trim_end_matches(';').trim();
        if text.is_empty() {
            return Err(DashError::new(ErrKind::BadQuery, "the query is empty").fix("Write a Cypher read, for example MATCH (n) RETURN n LIMIT 10."));
        }
        cypher_read_only(text).map_err(DashError::read_only)?;
        let v = self.run(text, cancel)?;
        let mut f = parse_answer(&v, req.limit())?;
        f.meta.took_ms = took(started);
        f.meta.executed = text.to_string();
        Ok(f)
    }

    fn starter(&self, target: &str) -> String {
        if is_ident(target) { format!("MATCH (n:`{target}`) RETURN n LIMIT 100") } else { "MATCH (n) RETURN n LIMIT 100".into() }
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::connector::fake::*;
    use crate::wsexec::FakeRunner;
    use std::sync::Arc;

    fn src(up: &Arc<FakeUp>) -> Neo4j {
        let d = deps(up, &[], Arc::new(FakeRunner::new(&[])));
        Neo4j::new(&cfg("neo4j", "http://g:7474"), &d).unwrap()
    }

    const ROWS: &str = r#"{"data":{"fields":["name","calls","n"],"values":[["main",3,{"labels":["Symbol"],"properties":{"a":1}}],["run",null,[1,2]]]},"queryType":"r"}"#;

    #[test]
    fn rows_become_typed_columns_and_nested_values_become_text() {
        let up = FakeUp::new().on("POST", 200, ROWS);
        let f = src(&up).query(&QueryRequest::new("MATCH (s:Symbol) RETURN s.name AS name, 3 AS calls, s AS n", range()), &Cancel::new()).unwrap();
        assert_eq!(f.columns.len(), 3);
        assert_eq!(f.cell(0, 0), "main");
        assert!(f.cell(0, 2).contains("Symbol"), "{}", f.cell(0, 2));
        assert_eq!(f.cell(1, 2), "[1,2]");
        let url = up.urls().remove(0);
        assert!(url.contains("/db/neo4j/query/v2"), "{url}");
        let body = up.bodies().remove(0);
        assert!(body.contains("\"accessMode\":\"Read\""), "every query is a read transaction: {body}");
    }

    #[test]
    fn rows_are_capped() {
        let values: Vec<String> = (0..50).map(|i| format!("[{i}]")).collect();
        let body = format!(r#"{{"data":{{"fields":["i"],"values":[{}]}}}}"#, values.join(","));
        let up = FakeUp::new().on("POST", 200, &body);
        let mut r = QueryRequest::new("MATCH (n) RETURN 1 AS i", range());
        r.limit = 10;
        let f = src(&up).query(&r, &Cancel::new()).unwrap();
        assert_eq!(f.rows.len(), 10);
        assert!(f.meta.truncated && f.meta.notices.iter().any(|n| n.contains("cut at 10")));
    }

    #[test]
    fn writes_never_leave_the_machine() {
        let up = FakeUp::new();
        let s = src(&up);
        for q in [
            "CREATE (n:X)",
            "MATCH (n) DELETE n",
            "MATCH (a)--(b) DETACH DELETE b",
            "MATCH (n) SET n.x = 1 RETURN n",
            "MATCH (n) REMOVE n.x",
            "MERGE (n:X)",
            "MATCH (n) RETURN n; MATCH (m) DELETE m",
            "CALL dbms.security.createUser('a','b')",
            "LOAD CSV FROM 'file:///etc/passwd' AS r RETURN r",
            "MATCH (n) CALL { WITH n CREATE (m) } RETURN n",
            "MATCH (n) /* x */ FOREACH (x IN [1] | CREATE (:Y))",
            "DROP DATABASE neo4j",
            "CREATE USER a SET PASSWORD 'b'",
            "USE system SHOW USERS",
        ] {
            let e = s.query(&QueryRequest::new(q, range()), &Cancel::new()).unwrap_err();
            assert_eq!(e.kind, ErrKind::ReadOnly, "{q}: {e}");
        }
        assert!(up.urls().is_empty(), "nothing was sent");
    }

    #[test]
    fn reads_that_look_like_writes_are_allowed() {
        assert!(cypher_read_only("MATCH (a)--(b) RETURN a.name, b.name LIMIT 5").is_ok());
        assert!(cypher_read_only("MATCH (a)-[:CALLS*1..3]->(b) WHERE a.name = 'delete me; create' RETURN b").is_ok());
        assert!(cypher_read_only("MATCH (n) WHERE n.set = 1 RETURN n // a comment: DELETE").is_ok());
        assert!(cypher_read_only("MATCH (n:`Create`) RETURN n;").is_ok());
        assert!(cypher_read_only("OPTIONAL MATCH (n) RETURN count(n)").is_ok());
        assert!(cypher_read_only("").is_err());
        assert!(cypher_read_only("MATCH (n) RETURN 'x").is_err());
    }

    #[test]
    fn server_errors_keep_their_message_and_never_the_password() {
        let up = FakeUp::new().on("POST", 400, r#"{"errors":[{"code":"Neo.ClientError.Statement.SyntaxError","message":"Invalid input 'X': expected a statement"}]}"#);
        let e = src(&up).query(&QueryRequest::new("MATCH (n RETURN n", range()), &Cancel::new()).unwrap_err();
        assert!(e.message.contains("Invalid input"), "{e}");
        let up = FakeUp::new().on("POST", 401, r#"{"errors":[{"code":"Neo.ClientError.Security.Unauthorized","message":"Invalid credential."}]}"#);
        assert_eq!(src(&up).query(&QueryRequest::new("RETURN 1", range()), &Cancel::new()).unwrap_err().kind, ErrKind::Auth);
        let up = FakeUp::new().on("POST", 200, r#"{"errors":[{"code":"Neo.ClientError.Statement.AccessMode","message":"Writing in read access mode not allowed."}]}"#);
        assert!(src(&up).query(&QueryRequest::new("RETURN 1", range()), &Cancel::new()).unwrap_err().message.contains("read access mode"));
    }

    #[test]
    fn test_reports_name_version_and_edition() {
        let up = FakeUp::new().on("POST", 200, r#"{"data":{"fields":["name","version","edition"],"values":[["Neo4j Kernel","2026.10.1","community"]]}}"#);
        assert_eq!(src(&up).test(&Cancel::new()).unwrap().detail, "Neo4j Kernel 2026.10.1 community");
    }

    #[test]
    fn browse_lists_labels_and_checks_the_name() {
        let up = FakeUp::new().on("db.labels", 200, r#"{"data":{"fields":["label"],"values":[["File"],["Symbol"]]}}"#).on("db.relationshipTypes", 200, r#"{"data":{"fields":["t"],"values":[["CALLS"]]}}"#);
        let n = src(&up).browse(&[], &Cancel::new()).unwrap();
        assert_eq!(n.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["File", "Symbol", "CALLS"]);
        assert!(src(&FakeUp::new()).browse(&["x` DETACH DELETE n //".into()], &Cancel::new()).is_err());
    }

    #[test]
    fn database_name_is_checked_and_odd_json_never_panics() {
        let mut c = cfg("neo4j", "http://g:7474");
        c.values.insert("database".into(), "../system".into());
        let d = deps(&FakeUp::new(), &[], Arc::new(FakeRunner::new(&[])));
        assert!(Neo4j::new(&c, &d).is_err());
        for body in ["", "null", "[]", "{}", r#"{"data":5}"#, r#"{"data":{"fields":[1],"values":[1,[],[null]]}}"#] {
            let up = FakeUp::new().on("POST", 200, body);
            let _ = src(&up).query(&QueryRequest::new("RETURN 1", range()), &Cancel::new());
        }
    }
}
