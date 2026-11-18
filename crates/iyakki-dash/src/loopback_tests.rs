//! Integration tests for the DASH connectors: the real connectors, the real HTTP client (`RealTransport`) and a small real HTTP
//! server on 127.0.0.1 that speaks the wire shapes of Prometheus, Elasticsearch, ClickHouse and Neo4j. The server records every
//! request (method, path, headers, body) so the tests check what really went over the socket, including the credential header.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use super::connector::*;
use iyakki_shared::wsexec::FakeRunner;

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, n: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str())
    }
}

type Handler = dyn Fn(&Seen) -> (u16, String) + Send + Sync;

struct Server {
    url: String,
    log: Arc<Mutex<Vec<Seen>>>,
}

impl Server {
    fn start(h: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static) -> Server {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let log: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let h: Arc<Handler> = Arc::new(h);
        let lg = log.clone();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let (h, lg) = (h.clone(), lg.clone());
                std::thread::spawn(move || serve(s, &*h, &lg));
            }
        });
        Server { url, log }
    }
    fn seen(&self) -> Vec<Seen> {
        self.log.lock().unwrap().clone()
    }
}

fn serve(s: std::net::TcpStream, h: &Handler, log: &Mutex<Vec<Seen>>) {
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut p = line.split_whitespace();
    let (method, target) = (p.next().unwrap_or("").to_string(), p.next().unwrap_or("").to_string());
    let mut headers = vec![];
    let mut len = 0usize;
    loop {
        let mut l = String::new();
        if r.read_line(&mut l).unwrap_or(0) == 0 || l.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = l.trim().split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let mut body = vec![0u8; len];
    let _ = r.read_exact(&mut body);
    let seen = Seen { method, target, headers, body: String::from_utf8_lossy(&body).into_owned() };
    let (code, text) = h(&seen);
    log.lock().unwrap().push(seen);
    let head = format!("HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", text.len());
    let mut s = s;
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(text.as_bytes());
}

struct Tok;
impl SecretResolver for Tok {
    fn resolve(&self, _: &str) -> Result<zeroize::Zeroizing<String>, String> {
        Ok(zeroize::Zeroizing::new("s3cret-token".into()))
    }
}

fn deps() -> Deps {
    Deps {
        transport: Arc::new(RealTransport),
        secrets: Arc::new(Tok),
        runner: Arc::new(FakeRunner::new(&[])),
        path_ok: Arc::new(|_| true),
        work_dir: std::env::temp_dir(),
        cluster: Arc::new(crate::cluster::NoCluster),
    }
}

fn source(kind: &str, url: &str, bearer: bool) -> Box<dyn DataSource> {
    let mut c = SourceConfig { id: "lb".into(), kind: kind.into(), name: "lb".into(), ..Default::default() };
    c.values.insert("url".into(), url.into());
    if bearer {
        c.values.insert("auth".into(), "bearer".into());
        c.secrets.insert("token".into(), "uvault:lb".into());
    }
    super::registry::build(&c, &deps()).unwrap()
}

fn range() -> TimeRange {
    TimeRange::last(3_600_000, 1_792_069_200_000)
}

#[test]
fn prometheus_over_a_real_socket_keeps_series_drops_nan_sends_the_token_and_keeps_the_servers_error() {
    let srv = Server::start(|r| {
        if r.target.starts_with("/api/v1/status/buildinfo") {
            (200, r#"{"status":"success","data":{"version":"2.53.0"}}"#.into())
        } else if r.target.starts_with("/api/v1/query") && r.target.contains("query=bad") {
            (400, r#"{"status":"error","errorType":"bad_data","error":"parse error: unexpected end of input"}"#.into())
        } else if r.target.starts_with("/api/v1/query_range") {
            (200, r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{"__name__":"up","job":"node"},"values":[[1792065600,"1"],[1792065660,"NaN"],[1792065720,"0"]]}]}}"#.into())
        } else if r.target.starts_with("/api/v1/label/__name__/values") {
            (200, r#"{"status":"success","data":["up","node_load1"]}"#.into())
        } else {
            (404, "{}".into())
        }
    });
    let s = source("prometheus", &srv.url, true);
    assert!(s.test(&Cancel::new()).unwrap().detail.contains("2.53.0"));
    let mut q = QueryRequest::new("up", range());
    q.step_ms = Some(60_000);
    let f = s.query(&q, &Cancel::new()).unwrap();
    assert_eq!(f.series.len(), 1);
    assert_eq!(f.series[0].points.iter().map(|p| p.1).collect::<Vec<_>>(), vec![1.0, 0.0], "NaN dropped");
    assert_eq!(s.browse(&[], &Cancel::new()).unwrap().len(), 2);
    let e = s.query(&QueryRequest::new("bad(", range()), &Cancel::new()).unwrap_err();
    assert!(e.message.contains("unexpected end of input"), "{}", e.message);
    let seen = srv.seen();
    let range_req = seen.iter().find(|r| r.target.contains("query=up")).unwrap();
    assert!(range_req.target.contains("step=60") && range_req.target.contains("start=1792065600.000"), "{}", range_req.target);
    assert!(seen.iter().all(|r| r.method == "GET" && r.header("authorization") == Some("Bearer s3cret-token")), "every request carries the token and only reads");
}

#[test]
fn elasticsearch_over_a_real_socket_posts_the_body_flattens_hits_and_names_the_cluster() {
    let srv = Server::start(|r| match (r.method.as_str(), r.target.split('?').next().unwrap()) {
        ("GET", "/") => (200, r#"{"cluster_name":"prod","version":{"number":"8.14.0"}}"#.into()),
        ("GET", "/_cluster/health") => (200, r#"{"status":"green"}"#.into()),
        ("GET", "/_cat/indices") => (200, r#"[{"index":"logs-1","docs.count":"12","store.size":"1kb","health":"green"}]"#.into()),
        ("POST", "/logs-1/_search") => (200, r#"{"took":3,"hits":{"total":{"value":2,"relation":"eq"},"hits":[{"_index":"logs-1","_id":"a","_source":{"msg":"hi","http":{"status":200}}},{"_index":"logs-1","_id":"b","_source":{"msg":"yo","http":{"status":500}}}]}}"#.into()),
        ("POST", "/logs-1/_search/bad") => (400, "{}".into()),
        _ => (404, r#"{"error":{"reason":"no such thing"}}"#.into()),
    });
    let s = source("elasticsearch", &srv.url, true);
    assert_eq!(s.test(&Cancel::new()).unwrap().detail, "Elasticsearch 8.14.0 cluster prod (green)");
    assert_eq!(s.browse(&[], &Cancel::new()).unwrap()[0].name, "logs-1");
    let mut q = QueryRequest::new(r#"{"query":{"match_all":{}},"size":2}"#, range());
    q.target = Some("logs-1".into());
    let f = s.query(&q, &Cancel::new()).unwrap();
    assert_eq!(f.rows.len(), 2);
    assert!(f.columns.iter().any(|c| c.name == "http.status"), "nested fields are flattened: {:?}", f.columns.iter().map(|c| &c.name).collect::<Vec<_>>());
    let post = srv.seen().into_iter().find(|r| r.method == "POST").unwrap();
    assert!(post.body.contains("match_all"), "the query text is the body: {}", post.body);
    assert_eq!(post.header("authorization"), Some("Bearer s3cret-token"));
    let mut missing = QueryRequest::new(r#"{"query":{"match_all":{}}}"#, range());
    missing.target = Some("nope".into());
    assert!(s.query(&missing, &Cancel::new()).is_err(), "an unknown index is an error, not an empty frame");
}

#[test]
fn clickhouse_over_a_real_socket_posts_the_sql_read_only_and_shows_an_exception_inside_a_200() {
    let srv = Server::start(|r| {
        if r.body.contains("FROM boom") {
            return (200, r#"{"exception":"Code: 60. DB::Exception: Table default.boom does not exist"}"#.into());
        }
        if r.body.contains("version()") {
            return (200, r#"{"meta":[{"name":"v","type":"String"},{"name":"u","type":"String"}],"data":[{"v":"24.3","u":"default"}],"rows":1}"#.into());
        }
        (200, r#"{"meta":[{"name":"t","type":"DateTime"},{"name":"cpu","type":"Float64"},{"name":"n","type":"UInt64"}],"data":[{"t":"2026-11-01 10:00:00","cpu":1.5,"n":18446744073709},{"t":"2026-11-01 10:01:00","cpu":2.5,"n":2}],"rows":2,"statistics":{"elapsed":0.01}}"#.into())
    });
    let s = source("clickhouse", &srv.url, false);
    assert!(s.test(&Cancel::new()).unwrap().detail.contains("24.3"));
    let f = s.query(&QueryRequest::new("SELECT t, cpu, n FROM m", range()), &Cancel::new()).unwrap();
    assert_eq!(f.rows.len(), 2);
    assert!(!f.series.is_empty(), "a time column turns into series");
    let e = s.query(&QueryRequest::new("SELECT * FROM boom", range()), &Cancel::new()).unwrap_err();
    assert!(e.message.contains("does not exist"), "{}", e.message);
    let before = srv.seen().len();
    for w in ["DROP TABLE m", "INSERT INTO m VALUES (1)"] {
        assert!(s.query(&QueryRequest::new(w, range()), &Cancel::new()).is_err());
    }
    assert_eq!(srv.seen().len(), before, "a write never reaches the server");
    let sel = srv.seen().into_iter().find(|r| r.body.starts_with("SELECT t, cpu")).unwrap();
    assert_eq!(sel.method, "POST");
    for want in ["default_format=JSON", "readonly=2", "result_overflow_mode=break", "max_result_rows="] {
        assert!(sel.target.contains(want), "{want} in {}", sel.target);
    }
}

#[test]
fn neo4j_over_a_real_socket_sends_every_query_as_a_read_transaction() {
    let srv = Server::start(|r| {
        if r.header("authorization").is_none() {
            return (401, r#"{"errors":[{"code":"Neo.ClientError.Security.Unauthorized","message":"Invalid credential."}]}"#.into());
        }
        (200, r#"{"data":{"fields":["name","n"],"values":[["main",3],["run",null]]},"queryType":"r"}"#.into())
    });
    let s = source("neo4j", &srv.url, true);
    let f = s.query(&QueryRequest::new("MATCH (s:Symbol) RETURN s.name AS name, 3 AS n", range()), &Cancel::new());
    let seen = srv.seen();
    assert!(!seen.is_empty());
    assert_eq!(seen[0].method, "POST");
    assert!(seen[0].target.contains("/db/neo4j/query/v2"), "{}", seen[0].target);
    assert!(seen[0].body.contains("\"accessMode\":\"Read\""), "{}", seen[0].body);
    let f = f.unwrap();
    assert_eq!(f.rows.len(), 2);
    assert_eq!(f.cell(0, 0), "main");
    assert_eq!(seen[0].header("authorization"), Some("Bearer s3cret-token"));
    let before = srv.seen().len();
    assert!(s.query(&QueryRequest::new("MATCH (n) DETACH DELETE n", range()), &Cancel::new()).is_err());
    assert_eq!(srv.seen().len(), before, "a write never reaches the server");
}

#[test]
fn a_dead_port_is_a_calm_sentence_not_a_panic() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    drop(l);
    for kind in ["prometheus", "elasticsearch", "clickhouse"] {
        let s = source(kind, &url, false);
        let e = s.query(&QueryRequest::new("SELECT 1", range()), &Cancel::new()).unwrap_err();
        assert!(!e.message.is_empty());
    }
}
