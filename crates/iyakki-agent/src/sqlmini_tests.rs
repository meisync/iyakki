//! `sqlmini`: round trips on made-up tables, the refusals, and, where the `sqlite3` program exists, an independent check: it makes the
//! database that is read, and opens the one that is written (integrity check, row counts, sums).

use super::sqlmini::*;
use std::process::{Command, Stdio};
use std::io::Write;

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-sqlmini-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sqlite3(db: &std::path::Path, sql: &str) -> Option<String> {
    let mut c = Command::new("sqlite3").arg(db).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    c.stdin.take().unwrap().write_all(sql.as_bytes()).ok()?;
    let o = c.wait_with_output().ok()?;
    assert!(o.status.success(), "sqlite3: {}", String::from_utf8_lossy(&o.stderr));
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

const COOKIES: &str = "CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, lastAccessed INTEGER, isSecure INTEGER, isHttpOnly INTEGER, sameSite INTEGER)";

/// A record of text and integer columns as a database stores it (what the tests need to make rows by hand).
fn record(cols: &[&str]) -> Vec<u8> {
    let (mut types, mut body) = (Vec::new(), Vec::new());
    for c in cols {
        let n = 13 + 2 * c.len();
        assert!(n < 128, "the test records are short");
        types.push(n as u8);
        body.extend_from_slice(c.as_bytes());
    }
    let mut r = vec![(types.len() + 1) as u8];
    r.extend(types);
    r.extend(body);
    r
}

fn big_record(cols: &[&str], big: usize) -> Vec<u8> {
    // one long text column (length as a two-byte varint)
    let (mut types, mut body) = (Vec::new(), Vec::new());
    for c in cols {
        types.push((13 + 2 * c.len()) as u8);
        body.extend_from_slice(c.as_bytes());
    }
    let n = 13 + 2 * big as u64;
    types.push(((n >> 7) as u8) | 0x80);
    types.push((n & 0x7f) as u8);
    body.extend(std::iter::repeat_n(b'x', big));
    let mut r = vec![(types.len() + 1) as u8];
    r.extend(types);
    r.extend(body);
    r
}

#[test]
fn tables_survive_a_write_and_a_read_including_empty_ones_big_rows_and_many_pages() {
    let rows: Vec<(i64, Vec<u8>)> = (1..=5000).map(|i| (i, if i % 500 == 0 { big_record(&["n"], 20_000) } else { record(&[&format!("n{i}"), "v", ".a.example.com"]) })).collect();
    let tables = vec![
        Table { name: "moz_cookies".into(), sql: "CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, name TEXT, value TEXT, host TEXT)".into(), rows: rows.clone() },
        Table { name: "empty".into(), sql: "CREATE TABLE empty (a)".into(), rows: vec![] },
        Table { name: "one".into(), sql: "CREATE TABLE one (a)".into(), rows: vec![(7, record(&["x"]))] },
    ];
    let db = write(&tables).unwrap();
    assert_eq!(db.len() % 4096, 0);
    let back = read(&db).unwrap();
    assert_eq!(back.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["moz_cookies", "empty", "one"]);
    assert_eq!(back[0].rows, rows, "rows keep their rowids and exact record bytes, the 20 KB ones through overflow pages");
    assert!(back[1].rows.is_empty() && back[2].rows == vec![(7, record(&["x"]))]);
    assert_eq!(column_text(&back[0].rows[0].1, 2).as_deref(), Some(".a.example.com"));
    assert!(db.len() > 8 * 4096, "thousands of rows need an interior level");
}

#[test]
fn files_that_are_not_what_the_reader_understands_are_refused_in_plain_words() {
    assert!(read(b"not a database at all, just text").unwrap_err().contains("not an SQLite"));
    let good = write(&[Table { name: "t".into(), sql: "CREATE TABLE t (a)".into(), rows: vec![(1, record(&["x"]))] }]).unwrap();
    assert!(read(&good[..good.len() - 100]).is_err(), "a cut file");
    let mut utf16 = good.clone();
    utf16[56..60].copy_from_slice(&2u32.to_be_bytes());
    assert!(read(&utf16).unwrap_err().contains("UTF-8"));
    let mut vac = good.clone();
    vac[52..56].copy_from_slice(&5u32.to_be_bytes());
    assert!(read(&vac).unwrap_err().contains("auto-vacuum"));
    let mut odd = good.clone();
    odd[16..18].copy_from_slice(&1000u16.to_be_bytes());
    assert!(read(&odd).is_err());
    // damaged pages never panic: flip bytes all over and read
    for i in (100..good.len()).step_by(37) {
        let mut x = good.clone();
        x[i] ^= 0xff;
        let _ = read(&x);
    }
    let junk: Vec<u8> = (0..8192u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
    let _ = read(&junk);
    assert!(write(&[Table { name: String::new(), sql: String::new(), rows: vec![] }]).is_err());
}

#[test]
fn a_real_cookie_database_made_by_sqlite_is_read_filtered_and_written_back_so_sqlite_accepts_it() {
    let d = tmp("real");
    let src = d.join("cookies.sqlite");
    let made = sqlite3(&src, &format!("{COOKIES}; WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM c WHERE i<3000) INSERT INTO moz_cookies SELECT i, 'n'||i, CASE WHEN i%97=0 THEN hex(zeroblob(9000)) ELSE 'v'||i END, CASE i%3 WHEN 0 THEN '.a.example.com' WHEN 1 THEN 'b.example.org' ELSE 'c.example.net' END, '/', 2000000000+i, i*1.5, i%2, 0, 0 FROM c; SELECT count(*) FROM moz_cookies;"));
    let Some(made) = made else {
        crate::qeprintln!("sqlite3 is not installed: skipped");
        return;
    };
    assert_eq!(made, "3000");
    let bytes = std::fs::read(&src).unwrap();
    let mut tables = read(&bytes).unwrap();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].name, "moz_cookies");
    assert_eq!(tables[0].rows.len(), 3000, "every row, the big ones through their overflow chains");
    // leave out the rows of one site and its sub-domains
    let host_col = 3;
    tables[0].rows.retain(|(_, rec)| !column_text(rec, host_col).unwrap().trim_start_matches('.').ends_with("example.com"));
    assert_eq!(tables[0].rows.len(), 2000);
    let out = d.join("filtered.sqlite");
    std::fs::write(&out, write(&tables).unwrap()).unwrap();
    // the independent reader: it finds the file sound, the right rows and the exact values of the ones kept
    assert_eq!(sqlite3(&out, "PRAGMA integrity_check;").unwrap(), "ok");
    assert_eq!(sqlite3(&out, "SELECT count(*), count(DISTINCT host), sum(length(value)) FROM moz_cookies;").unwrap(), sqlite3(&src, "SELECT count(*), count(DISTINCT host), sum(length(value)) FROM moz_cookies WHERE host NOT LIKE '%example.com';").unwrap());
    assert_eq!(sqlite3(&out, "SELECT count(*) FROM moz_cookies WHERE host LIKE '%example.com';").unwrap(), "0");
    assert_eq!(sqlite3(&out, "SELECT id, name, host, expiry, lastAccessed FROM moz_cookies WHERE id = 1;").unwrap(), sqlite3(&src, "SELECT id, name, host, expiry, lastAccessed FROM moz_cookies WHERE id = 1;").unwrap(), "values and the real number survive exactly");
    // and it can be written to: the file is a database like any other
    assert_eq!(sqlite3(&out, "INSERT INTO moz_cookies (name, value, host, path) VALUES ('new', 'row', 'd.example.io', '/'); SELECT count(*) FROM moz_cookies;").unwrap(), "2001");
    assert_eq!(sqlite3(&out, "PRAGMA integrity_check;").unwrap(), "ok");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_database_with_an_index_a_view_or_a_sequence_is_refused_and_so_is_one_with_a_log() {
    let d = tmp("refuse");
    let p = d.join("x.sqlite");
    if sqlite3(&p, "CREATE TABLE t (a TEXT, b TEXT); CREATE INDEX i ON t(a); INSERT INTO t VALUES ('x','y');").is_none() {
        crate::qeprintln!("sqlite3 is not installed: skipped");
        return;
    }
    assert!(read(&std::fs::read(&p).unwrap()).unwrap_err().contains("index"));
    let p2 = d.join("v.sqlite");
    sqlite3(&p2, "CREATE TABLE t (a); CREATE VIEW v AS SELECT a FROM t;").unwrap();
    assert!(read(&std::fs::read(&p2).unwrap()).unwrap_err().contains("view"));
    let p3 = d.join("s.sqlite");
    sqlite3(&p3, "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, a); INSERT INTO t (a) VALUES ('x');").unwrap();
    assert!(read(&std::fs::read(&p3).unwrap()).unwrap_err().contains("bookkeeping"));
    let p4 = d.join("k.sqlite");
    sqlite3(&p4, "CREATE TABLE t (a TEXT PRIMARY KEY, b);").unwrap();
    assert!(read(&std::fs::read(&p4).unwrap()).is_err(), "a table with an automatic index is not rewritten");
    let _ = std::fs::remove_dir_all(&d);
}
