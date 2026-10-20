//! Line coverage per file and overall, read from the report of a coverage tool, and mapped onto features through their `Files`.
//!
//! Supported reports: lcov (`SF:`/`DA:`), `coverage json` (coverage.py), `cargo llvm-cov --json`, `cargo tarpaulin --out Json`,
//! jest/c8 `json-summary`, and the Go cover profile. Nothing is invented: a feature without `Files` has no number. A tool that is
//! not installed is never run; [`plan`] returns the exact install hint instead.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

/// Lines covered and lines in total, per file (paths as the tool printed them, made relative to the project when possible).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub files: BTreeMap<String, (u64, u64)>,
    /// Unix seconds of the run.
    pub ran_at: i64,
}

impl Coverage {
    /// Overall line coverage in percent, when any lines were counted.
    pub fn overall(&self) -> Option<f32> {
        let (c, t) = self.files.values().fold((0u64, 0u64), |a, f| (a.0 + f.0, a.1 + f.1));
        (t > 0).then(|| c as f32 * 100.0 / t as f32)
    }

    /// Covered and total lines of the files matching any of the globs; None when the feature names no file or none matches.
    pub fn for_globs(&self, globs: &[String]) -> Option<(u64, u64)> {
        if globs.is_empty() {
            return None;
        }
        let (mut c, mut t, mut any) = (0u64, 0u64, false);
        for (f, (cv, tot)) in &self.files {
            if globs.iter().any(|g| glob_match(g, f)) {
                c += cv;
                t += tot;
                any = true;
            }
        }
        (any && t > 0).then_some((c, t))
    }

    pub fn to_json(&self) -> Value {
        json!({"ran_at": self.ran_at, "files": self.files.iter().map(|(k, (c, t))| (k.clone(), json!([c, t]))).collect::<serde_json::Map<_, _>>()})
    }

    pub fn from_json(v: &Value) -> Option<Coverage> {
        let files = v.get("files")?.as_object()?.iter().filter_map(|(k, x)| Some((k.clone(), (x.get(0)?.as_u64()?, x.get(1)?.as_u64()?)))).collect();
        Some(Coverage { files, ran_at: v.get("ran_at").and_then(Value::as_i64).unwrap_or(0) })
    }
}

pub fn save(root: &Path, c: &Coverage) {
    let p = root.join(".iyakki/coverage.json");
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, c.to_json().to_string());
}

pub fn load(root: &Path) -> Option<Coverage> {
    serde_json::from_str::<Value>(&std::fs::read_to_string(root.join(".iyakki/coverage.json")).ok()?).ok().and_then(|v| Coverage::from_json(&v))
}

fn rel(root: Option<&Path>, p: &str) -> String {
    let p = p.trim_start_matches("./");
    match root.and_then(|r| p.strip_prefix(&format!("{}/", r.display()))) {
        Some(x) => x.to_string(),
        None => p.to_string(),
    }
}

/// `*` within a name, `**` across folders, `?` one character; a pattern without wildcards also matches by path suffix.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat = pattern.trim().trim_start_matches("./");
    if pat.is_empty() {
        return false;
    }
    if !pat.contains(['*', '?']) {
        return path == pat || path.ends_with(&format!("/{pat}")) || (pat.ends_with('/') && path.starts_with(pat));
    }
    fn m(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => {
                let double = p.get(1) == Some(&b'*');
                let rest = if double { &p[2..] } else { &p[1..] };
                let rest = if double && rest.first() == Some(&b'/') { &rest[1..] } else { rest };
                (0..=s.len()).any(|i| (double || !s[..i].contains(&b'/')) && m(rest, &s[i..]))
            }
            (Some(b'?'), Some(c)) => *c != b'/' && m(&p[1..], &s[1..]),
            (Some(a), Some(b)) => a == b && m(&p[1..], &s[1..]),
            _ => false,
        }
    }
    m(pat.as_bytes(), path.as_bytes()) || path.split('/').enumerate().any(|(i, _)| { let tail = path.splitn(i + 1, '/').last().unwrap_or(""); i > 0 && m(pat.as_bytes(), tail.as_bytes()) })
}

pub fn parse_lcov(text: &str, root: Option<&Path>) -> Coverage {
    let mut c = Coverage::default();
    let (mut file, mut cov, mut tot) = (String::new(), 0u64, 0u64);
    for l in text.lines() {
        if let Some(f) = l.strip_prefix("SF:") {
            file = rel(root, f.trim());
            (cov, tot) = (0, 0);
        } else if let Some(d) = l.strip_prefix("DA:") {
            let hits = d.split(',').nth(1).and_then(|h| h.trim().parse::<u64>().ok()).unwrap_or(0);
            tot += 1;
            cov += (hits > 0) as u64;
        } else if l == "end_of_record" && !file.is_empty() {
            c.files.insert(std::mem::take(&mut file), (cov, tot));
        }
    }
    c
}

pub fn parse_go_profile(text: &str) -> Coverage {
    let mut c = Coverage::default();
    for l in text.lines().skip_while(|l| l.starts_with("mode:")) {
        // file.go:10.2,12.3 2 1
        let Some((name, rest)) = l.split_once(':') else { continue };
        let mut w = rest.split_whitespace();
        let (_, n, hits) = (w.next(), w.next().and_then(|n| n.parse::<u64>().ok()), w.next().and_then(|n| n.parse::<u64>().ok()));
        if let (Some(n), Some(h)) = (n, hits) {
            let e = c.files.entry(name.to_string()).or_insert((0, 0));
            e.1 += n;
            e.0 += if h > 0 { n } else { 0 };
        }
    }
    c
}

/// Any JSON report: coverage.py, llvm-cov, tarpaulin or jest/c8 json-summary.
pub fn parse_json(v: &Value, root: Option<&Path>) -> Coverage {
    let mut c = Coverage::default();
    // coverage.py: {"files": {"a.py": {"summary": {"covered_lines": 8, "num_statements": 10}}}}
    if let Some(files) = v.get("files").and_then(Value::as_object) {
        for (k, f) in files {
            if let Some(s) = f.get("summary") {
                if let (Some(cv), Some(t)) = (s.get("covered_lines").and_then(Value::as_u64), s.get("num_statements").and_then(Value::as_u64)) {
                    c.files.insert(rel(root, k), (cv, t));
                }
            }
        }
    }
    // llvm-cov: {"data": [{"files": [{"filename": "...", "summary": {"lines": {"count": 10, "covered": 8}}}]}]}
    for d in v.get("data").and_then(Value::as_array).into_iter().flatten() {
        for f in d.get("files").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(n), Some(l)) = (f.get("filename").and_then(Value::as_str), f.pointer("/summary/lines")) {
                if let (Some(cv), Some(t)) = (l.get("covered").and_then(Value::as_u64), l.get("count").and_then(Value::as_u64)) {
                    c.files.insert(rel(root, n), (cv, t));
                }
            }
        }
    }
    // tarpaulin: {"files": [{"path": ["src", "lib.rs"], "covered": 8, "coverable": 10}]}
    for f in v.get("files").and_then(Value::as_array).into_iter().flatten() {
        let path = f.get("path").and_then(Value::as_array).map(|p| p.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("/")).or_else(|| f.get("path").and_then(Value::as_str).map(str::to_string));
        if let (Some(p), Some(cv), Some(t)) = (path, f.get("covered").and_then(Value::as_u64), f.get("coverable").and_then(Value::as_u64)) {
            c.files.insert(rel(root, &p), (cv, t));
        }
    }
    // jest / c8 json-summary: {"total": {...}, "/abs/path.js": {"lines": {"total": 10, "covered": 8}}}
    if let Some(o) = v.as_object() {
        for (k, f) in o.iter().filter(|(k, _)| *k != "total" && *k != "files" && *k != "data") {
            if let Some(l) = f.get("lines") {
                if let (Some(cv), Some(t)) = (l.get("covered").and_then(Value::as_u64), l.get("total").and_then(Value::as_u64)) {
                    c.files.insert(rel(root, k), (cv, t));
                }
            }
        }
    }
    c
}

/// Parse a report of any supported format.
pub fn parse_any(text: &str, root: Option<&Path>) -> Option<Coverage> {
    let t = text.trim_start();
    let c = if t.starts_with('{') {
        parse_json(&serde_json::from_str::<Value>(t).ok()?, root)
    } else if t.contains("SF:") {
        parse_lcov(t, root)
    } else if t.starts_with("mode:") {
        parse_go_profile(t)
    } else {
        return None;
    };
    (!c.files.is_empty()).then_some(c)
}

/// How to measure coverage in a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub argv: Vec<String>,
    /// A report file the command writes (read after it ends); None when the report is printed.
    pub out_file: Option<String>,
}

fn on_path(prog: &str) -> bool {
    std::env::var("PATH").unwrap_or_default().split(':').any(|d| !d.is_empty() && Path::new(d).join(prog).is_file())
}

/// The coverage command for a project: the configured one, else the first tool found for its language. `installed` tells whether a
/// program exists (tests pass a fake). Err is the exact hint for what to install.
pub fn plan(root: &Path, configured: &str, installed: &dyn Fn(&str) -> bool) -> Result<Plan, String> {
    if !configured.trim().is_empty() {
        let argv: Vec<String> = crate::wsexec::split_command(configured);
        let prog = argv.first().cloned().unwrap_or_default();
        return if installed(&prog) { Ok(Plan { argv, out_file: None }) } else { Err(format!("The coverage command `{configured}` needs `{prog}`, which is not installed on this machine.")) };
    }
    let has = |f: &str| root.join(f).exists();
    if has("Cargo.toml") {
        if installed("cargo-llvm-cov") {
            return Ok(Plan { argv: ["cargo", "llvm-cov", "--summary-only", "--json"].map(String::from).to_vec(), out_file: None });
        }
        if installed("cargo-tarpaulin") {
            return Ok(Plan { argv: ["cargo", "tarpaulin", "--out", "Json", "--output-dir", "target/tarpaulin"].map(String::from).to_vec(), out_file: Some("target/tarpaulin/tarpaulin-report.json".into()) });
        }
        return Err("No Rust coverage tool is installed. Install one: `cargo install cargo-llvm-cov` (and `rustup component add llvm-tools-preview`), or `cargo install cargo-tarpaulin`; or set \"commands\": {\"coverage\": \"…\"} in .iyakki/project.json.".into());
    }
    if has("go.mod") {
        return if installed("go") { Ok(Plan { argv: ["go", "test", "-coverprofile=cover.out", "./..."].map(String::from).to_vec(), out_file: Some("cover.out".into()) }) } else { Err("`go` is not installed: install Go (https://go.dev/dl) to measure coverage.".into()) };
    }
    if has("package.json") {
        let local = |t: &str| root.join("node_modules/.bin").join(t).exists();
        if local("jest") || local("vitest") {
            let tool = if local("jest") { "jest" } else { "vitest" };
            let mut argv = vec!["npx".to_string(), "--no-install".into(), tool.into()];
            argv.extend(if tool == "jest" { vec!["--coverage".to_string(), "--coverageReporters=json-summary".into()] } else { vec!["run".to_string(), "--coverage".into(), "--coverage.reporter=json-summary".into()] });
            return Ok(Plan { argv, out_file: Some("coverage/coverage-summary.json".into()) });
        }
        if local("c8") {
            return Ok(Plan { argv: ["npx", "--no-install", "c8", "--reporter=json-summary", "npm", "test"].map(String::from).to_vec(), out_file: Some("coverage/coverage-summary.json".into()) });
        }
        return Err("No Node coverage tool is installed in this project. Install one: `npm install --save-dev c8` (works with any test runner), or use jest with `--coverage`.".into());
    }
    if has("pyproject.toml") || has("requirements.txt") || has("setup.py") || has("pytest.ini") {
        return if installed("coverage") || installed("pytest-cov") {
            Ok(Plan { argv: ["python3", "-m", "pytest", "--cov", "--cov-report=json:coverage.json"].map(String::from).to_vec(), out_file: Some("coverage.json".into()) })
        } else {
            Err("No Python coverage tool is installed. Install one: `pip install pytest-cov` (or `pip install coverage`).".into())
        };
    }
    Err("No coverage tool is known for this project. Set \"commands\": {\"coverage\": \"…\"} in .iyakki/project.json (a command that prints an lcov or JSON report, or writes one the board can read).".into())
}

/// `plan` with the real PATH.
pub fn plan_here(root: &Path, configured: &str) -> Result<Plan, String> {
    plan(root, configured, &|p| on_path(p))
}

/// Read the report of a finished coverage run (the file it wrote, or what it printed) and store it.
pub fn record_run(root: &Path, plan: &Plan, stdout: &str, now: i64) -> Result<Coverage, String> {
    let text = match &plan.out_file {
        Some(f) => std::fs::read_to_string(root.join(f)).map_err(|_| format!("the coverage run did not write {f}"))?,
        None => stdout.to_string(),
    };
    let mut c = parse_any(&text, Some(root)).ok_or("the coverage report could not be read (no lcov, JSON or Go profile found)")?;
    c.ran_at = now;
    save(root, &c);
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_common_report_format_gives_lines_per_file() {
        let lcov = "TN:\nSF:/p/src/cart.js\nDA:1,1\nDA:2,0\nDA:3,4\nend_of_record\nSF:/p/src/tax.js\nDA:1,0\nend_of_record\n";
        let c = parse_lcov(lcov, Some(Path::new("/p")));
        assert_eq!((c.files["src/cart.js"], c.files["src/tax.js"]), ((2, 3), (0, 1)));
        let go = "mode: set\nshop/cart.go:10.2,12.3 2 1\nshop/cart.go:14.2,15.3 1 0\nshop/tax.go:3.1,4.2 3 5\n";
        let g = parse_go_profile(go);
        assert_eq!((g.files["shop/cart.go"], g.files["shop/tax.go"]), ((2, 3), (3, 3)));
        let py: Value = serde_json::from_str(r#"{"files": {"shop/cart.py": {"summary": {"covered_lines": 8, "num_statements": 10}}}, "totals": {}}"#).unwrap();
        assert_eq!(parse_json(&py, None).files["shop/cart.py"], (8, 10));
        let llvm: Value = serde_json::from_str(r#"{"data": [{"files": [{"filename": "/p/src/lib.rs", "summary": {"lines": {"count": 20, "covered": 15}}}], "totals": {}}]}"#).unwrap();
        assert_eq!(parse_json(&llvm, Some(Path::new("/p"))).files["src/lib.rs"], (15, 20));
        let tarp: Value = serde_json::from_str(r#"{"files": [{"path": ["", "p", "src", "lib.rs"], "covered": 5, "coverable": 10}]}"#).unwrap();
        assert_eq!(parse_json(&tarp, None).files["/p/src/lib.rs"], (5, 10));
        let jest: Value = serde_json::from_str(r#"{"total": {"lines": {"total": 99, "covered": 1}}, "/p/src/a.js": {"lines": {"total": 4, "covered": 3}}}"#).unwrap();
        let j = parse_json(&jest, Some(Path::new("/p")));
        assert_eq!((j.files.len(), j.files["src/a.js"]), (1, (3, 4)), "the total entry is not a file");
        assert!(parse_any("nothing useful", None).is_none());
        assert_eq!(parse_any(lcov, Some(Path::new("/p"))).unwrap().files.len(), 2);
    }

    #[test]
    fn features_get_coverage_only_from_the_files_they_name_and_nothing_is_invented() {
        let c = Coverage { files: [("src/cart.js", (8, 10)), ("src/tax.js", (1, 10)), ("src/ui/menu.js", (5, 5))].map(|(k, v)| (k.to_string(), v)).into(), ran_at: 0 };
        assert_eq!(c.overall().map(|p| p.round()), Some(56.0));
        assert_eq!(c.for_globs(&["src/cart.js".into()]), Some((8, 10)));
        assert_eq!(c.for_globs(&["cart.js".into()]), Some((8, 10)), "a plain name matches by path suffix");
        assert_eq!(c.for_globs(&["src/*.js".into()]), Some((9, 20)), "* stays inside one folder");
        assert_eq!(c.for_globs(&["src/**/*.js".into()]), Some((14, 25)));
        assert_eq!(c.for_globs(&["src/ui/".into()]), Some((5, 5)));
        assert_eq!(c.for_globs(&[]), None, "a feature without Files has no number");
        assert_eq!(c.for_globs(&["lib/none.js".into()]), None);
        assert_eq!(Coverage::default().overall(), None);
        let back = Coverage::from_json(&c.to_json()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn the_command_is_chosen_by_language_and_a_missing_tool_is_never_run_but_explained() {
        let d = std::env::temp_dir().join(format!("iyakki-cov-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let none = |_: &str| false;
        let all = |_: &str| true;
        assert!(plan(&d, "", &none).unwrap_err().contains("No coverage tool is known"));
        std::fs::write(d.join("Cargo.toml"), "[package]").unwrap();
        let e = plan(&d, "", &none).unwrap_err();
        assert!(e.contains("cargo install cargo-llvm-cov") && e.contains("llvm-tools-preview") && e.contains("tarpaulin"), "{e}");
        assert_eq!(plan(&d, "", &all).unwrap().argv, ["cargo", "llvm-cov", "--summary-only", "--json"]);
        assert_eq!(plan(&d, "", &|p: &str| p == "cargo-tarpaulin").unwrap().out_file.as_deref(), Some("target/tarpaulin/tarpaulin-report.json"));
        std::fs::remove_file(d.join("Cargo.toml")).unwrap();
        std::fs::write(d.join("package.json"), "{}").unwrap();
        assert!(plan(&d, "", &all).unwrap_err().contains("npm install --save-dev c8"));
        std::fs::create_dir_all(d.join("node_modules/.bin")).unwrap();
        std::fs::write(d.join("node_modules/.bin/c8"), "").unwrap();
        assert!(plan(&d, "", &all).unwrap().argv.contains(&"c8".to_string()));
        std::fs::remove_file(d.join("package.json")).unwrap();
        std::fs::write(d.join("go.mod"), "module x").unwrap();
        assert_eq!(plan(&d, "", &all).unwrap().out_file.as_deref(), Some("cover.out"));
        assert!(plan(&d, "", &none).unwrap_err().contains("install Go"));
        std::fs::remove_file(d.join("go.mod")).unwrap();
        std::fs::write(d.join("requirements.txt"), "pytest").unwrap();
        assert!(plan(&d, "", &none).unwrap_err().contains("pip install pytest-cov"));
        assert_eq!(plan(&d, "", &all).unwrap().out_file.as_deref(), Some("coverage.json"));
        // a configured command wins, and is not run when its program is missing
        assert_eq!(plan(&d, "mycov --lcov", &all).unwrap().argv, ["mycov", "--lcov"]);
        assert!(plan(&d, "mycov --lcov", &none).unwrap_err().contains("`mycov`, which is not installed"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_finished_run_is_read_from_its_file_or_its_output_and_stored() {
        let d = std::env::temp_dir().join(format!("iyakki-cov2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("coverage")).unwrap();
        std::fs::write(d.join("coverage/coverage-summary.json"), format!(r#"{{"total": {{}}, "{}/src/a.js": {{"lines": {{"total": 4, "covered": 2}}}}}}"#, d.display())).unwrap();
        let p = Plan { argv: vec![], out_file: Some("coverage/coverage-summary.json".into()) };
        let c = record_run(&d, &p, "", 77).unwrap();
        assert_eq!((c.files["src/a.js"], c.ran_at), ((2, 4), 77));
        assert_eq!(load(&d).unwrap(), c);
        assert!(record_run(&d, &Plan { argv: vec![], out_file: Some("missing.json".into()) }, "", 1).unwrap_err().contains("did not write"));
        assert!(record_run(&d, &Plan { argv: vec![], out_file: None }, "no report here", 1).unwrap_err().contains("could not be read"));
        assert_eq!(record_run(&d, &Plan { argv: vec![], out_file: None }, "SF:a.rs\nDA:1,1\nend_of_record\n", 5).unwrap().files["a.rs"], (1, 1));
        let _ = std::fs::remove_dir_all(&d);
    }
}
