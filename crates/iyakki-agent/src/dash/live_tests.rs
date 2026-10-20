//! One live test per connector. They talk to a real server, so they are `#[ignore]`d and only run when asked
//! (`cargo test -p iyakki-agent --lib dash::live -- --ignored`) with the server named in the environment:
//! `IYAKKI_LIVE_PROM`, `IYAKKI_LIVE_ES`, `IYAKKI_LIVE_CLICKHOUSE` (addresses), `IYAKKI_LIVE_DUCKDB` (a CSV or Parquet file).
//! Credentials, when the server needs them, come from `IYAKKI_LIVE_TOKEN` and are handed to the connector as a vault
//! reference through a one-entry resolver, exactly as in production.

use std::sync::Arc;

use super::connector::*;
use crate::wsexec::System;

struct Env1(String);
impl SecretResolver for Env1 {
    fn resolve(&self, _: &str) -> Result<zeroize::Zeroizing<String>, String> {
        Ok(zeroize::Zeroizing::new(self.0.clone()))
    }
}

fn deps() -> Deps {
    let token = std::env::var("IYAKKI_LIVE_TOKEN").unwrap_or_default();
    Deps {
        transport: Arc::new(RealTransport),
        secrets: Arc::new(Env1(token)),
        runner: Arc::new(System),
        path_ok: Arc::new(|_| true),
        work_dir: std::env::temp_dir(),
    }
}

fn cfg(kind: &str, url: &str) -> SourceConfig {
    let mut c = SourceConfig { id: "live".into(), kind: kind.into(), name: "live".into(), ..Default::default() };
    c.values.insert("url".into(), url.into());
    if std::env::var("IYAKKI_LIVE_TOKEN").is_ok() {
        c.values.insert("auth".into(), "bearer".into());
        c.secrets.insert("token".into(), "uvault:live".into());
    }
    c
}

fn range() -> TimeRange {
    let now = crate::clock::now_unix() * 1000;
    TimeRange::last(3_600_000, now)
}

#[test]
#[ignore = "needs a Prometheus server: set IYAKKI_LIVE_PROM"]
fn prometheus_live() {
    let Ok(url) = std::env::var("IYAKKI_LIVE_PROM") else { return };
    let s = super::registry::build(&cfg("prometheus", &url), &deps()).unwrap();
    println!("{}", s.test(&Cancel::new()).unwrap().detail);
    let mut r = QueryRequest::new("up", range());
    r.step_ms = Some(60_000);
    let f = s.query(&r, &Cancel::new()).unwrap();
    assert!(!f.series.is_empty());
    assert!(!s.browse(&[], &Cancel::new()).unwrap().is_empty());
}

#[test]
#[ignore = "needs an Elasticsearch server: set IYAKKI_LIVE_ES"]
fn elasticsearch_live() {
    let Ok(url) = std::env::var("IYAKKI_LIVE_ES") else { return };
    let s = super::registry::build(&cfg("elasticsearch", &url), &deps()).unwrap();
    println!("{}", s.test(&Cancel::new()).unwrap().detail);
    let idx = s.browse(&[], &Cancel::new()).unwrap();
    let mut r = QueryRequest::new("{\"query\":{\"match_all\":{}},\"size\":5}", range());
    r.target = Some(idx.first().map_or("*".to_string(), |n| n.name.clone()));
    s.query(&r, &Cancel::new()).unwrap();
}

#[test]
#[ignore = "needs a ClickHouse server: set IYAKKI_LIVE_CLICKHOUSE"]
fn clickhouse_live() {
    let Ok(url) = std::env::var("IYAKKI_LIVE_CLICKHOUSE") else { return };
    let s = super::registry::build(&cfg("clickhouse", &url), &deps()).unwrap();
    println!("{}", s.test(&Cancel::new()).unwrap().detail);
    let f = s.query(&QueryRequest::new("SELECT name, engine FROM system.tables LIMIT 5", range()), &Cancel::new()).unwrap();
    assert!(!f.rows.is_empty());
    assert!(s.query(&QueryRequest::new("DROP TABLE nothing", range()), &Cancel::new()).is_err());
}

#[test]
#[ignore = "needs the duckdb program and a data file: set IYAKKI_LIVE_DUCKDB"]
fn duckdb_live() {
    let Ok(file) = std::env::var("IYAKKI_LIVE_DUCKDB") else { return };
    let mut c = SourceConfig { id: "live".into(), kind: "duckdb".into(), name: "live".into(), ..Default::default() };
    c.values.insert("files".into(), file.clone());
    let s = super::registry::build(&c, &deps()).unwrap();
    println!("{}", s.test(&Cancel::new()).unwrap().detail);
    let f = s.query(&QueryRequest::new(&s.starter(&file), range()), &Cancel::new()).unwrap();
    assert!(!f.columns.is_empty());
}
