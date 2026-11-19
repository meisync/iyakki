//! Charter gate 6 (docs/development/CORE-COMPATIBILITY-DESIGN.md sections 6 and 8): every deprecated name has a removal date and an alias
//! test, and an entry whose date has passed fails the gate so that nobody keeps a deprecated thing forever by forgetting it.

use std::path::{Path, PathBuf};

const TABLE: &str = include_str!("../../../docs/development/deprecations.txt");
const KINDS: [&str; 6] = ["capability", "call", "field", "message", "file", "other"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(Debug, PartialEq, Eq)]
struct Row {
    name: String,
    kind: String,
    since: String,
    not_before: (i64, u32, u32),
    alias_test: String,
}

/// `YYYY-MM-DD` with a real calendar day.
fn parse_date(s: &str) -> Option<(i64, u32, u32)> {
    let mut it = s.split('-');
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let (y, m, d): (i64, u32, u32) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    ((1..=12).contains(&m) && d >= 1 && d <= days[(m - 1) as usize]).then_some((y, m, d))
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn today() -> (i64, u32, u32) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    civil_from_days(secs.div_euclid(86_400))
}

fn parse_table(text: &str) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let c: Vec<&str> = line.split('\t').collect();
        if c.len() != 6 || c[..5].iter().any(|x| x.trim().is_empty()) || c[5].trim().is_empty() {
            return Err(format!("line {}: six tab separated columns, none empty (name kind since not_before alias_test note)", i + 1));
        }
        if !KINDS.contains(&c[1]) {
            return Err(format!("line {}: kind {:?} is not one of {KINDS:?}", i + 1, c[1]));
        }
        let not_before = parse_date(c[3]).ok_or_else(|| format!("line {}: {:?} is not a date (YYYY-MM-DD)", i + 1, c[3]))?;
        if rows.iter().any(|r: &Row| r.name == c[0] && r.kind == c[1]) {
            return Err(format!("line {}: {} is listed twice", i + 1, c[0]));
        }
        rows.push(Row { name: c[0].into(), kind: c[1].into(), since: c[2].into(), not_before, alias_test: c[4].into() });
    }
    Ok(rows)
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "target") {
                rust_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Whether `crate::module::test` names a `fn test` in a file of that crate.
fn test_exists(path: &str, files: &[(String, String)]) -> bool {
    let Some(f) = path.rsplit("::").next() else { return false };
    let needle = format!("fn {f}(");
    files.iter().any(|(_, t)| t.contains(&needle))
}

#[test]
fn the_table_reads_and_a_bad_row_says_which_line() {
    assert!(parse_table(TABLE).is_ok(), "{:?}", parse_table(TABLE));
    let good = "old_name\tcall\t0.1.0-alpha.3\t2027-03-01\tiyakki-core-api::x::old_name_still_works\tuse new_name\n";
    let rows = parse_table(good).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].not_before, (2027, 3, 1));
    assert_eq!((rows[0].kind.as_str(), rows[0].since.as_str(), rows[0].name.as_str()), ("call", "0.1.0-alpha.3", "old_name"));
    for bad in [
        "a\tcall\tv\t2027-02-30\tt::t\tn\n",
        "a\tcall\tv\t27-02-03\tt::t\tn\n",
        "a\tfruit\tv\t2027-02-03\tt::t\tn\n",
        "a\tcall\tv\t2027-02-03\tt::t\n",
        "a\tcall\tv\t2027-02-03\t\tn\n",
        "a\tcall\tv\t2027-02-03\tt::t\tn\na\tcall\tv\t2027-02-03\tt::t\tn\n",
    ] {
        assert!(parse_table(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn dates_are_real_and_today_is_computed_correctly() {
    assert_eq!(parse_date("2028-02-29"), Some((2028, 2, 29)));
    assert_eq!(parse_date("2027-02-29"), None);
    assert_eq!(parse_date("2027-13-01"), None);
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    assert_eq!(civil_from_days(20_058), (2024, 12, 1));
    assert!(today().0 >= 2026);
}

#[test]
fn every_deprecated_name_has_an_alias_test_that_exists_and_none_is_past_its_date() {
    let rows = parse_table(TABLE).expect("docs/development/deprecations.txt");
    let mut paths = Vec::new();
    rust_files(&root().join("crates"), &mut paths);
    let files: Vec<(String, String)> = paths.iter().filter_map(|p| Some((p.display().to_string(), std::fs::read_to_string(p).ok()?))).collect();
    let now = today();
    let mut problems = Vec::new();
    for r in &rows {
        if !test_exists(&r.alias_test, &files) {
            problems.push(format!("{} {}: the alias test {} does not exist", r.kind, r.name, r.alias_test));
        }
        if r.not_before < now {
            problems.push(format!("{} {} (since {}) is past its removal date {:04}-{:02}-{:02}: remove it and its row, or move the date with a reason", r.kind, r.name, r.since, r.not_before.0, r.not_before.1, r.not_before.2));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
