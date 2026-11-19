//! The last result of each test, read from the output of the project's test command, and how it maps onto the feature board.
//!
//! [`parse_output`] understands the output of `cargo test`, `pytest -v` (and `-rA`), `go test -v`, jest and vitest (default
//! output, or `--json`) and [`parse_junit`] JUnit XML (read from a few conventional paths by [`read_junit`]). Results are a
//! name to pass/fail/skip map saved per workspace in `.iyakki/test-results.json`, so the board shows them after a restart.
//! A feature lists its tests in the Tests column; [`match_row`] finds each listed test in the results (exact name, then a
//! name that ends the other at a path boundary, then a name that contains the other).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Skip,
}

impl Outcome {
    fn word(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Skip => "skip",
        }
    }
    fn from_word(w: &str) -> Option<Outcome> {
        match w {
            "pass" => Some(Outcome::Pass),
            "fail" => Some(Outcome::Fail),
            "skip" => Some(Outcome::Skip),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Results {
    /// Unix seconds of the run.
    pub ran_at: i64,
    pub tests: BTreeMap<String, Outcome>,
}

/// What the results say about the tests one feature lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowTests {
    pub listed: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Listed tests that are not in the results (not run, or named differently).
    pub not_run: usize,
}

impl RowTests {
    /// Every listed test ran and passed (skipped ones do not count as passed).
    pub fn all_passed(&self) -> bool {
        self.listed > 0 && self.passed == self.listed
    }
    /// "12/14 pass", "3 not run", "2 fail": what the board shows.
    pub fn label(&self) -> String {
        if self.listed == 0 {
            return String::new();
        }
        if self.failed > 0 {
            format!("{}/{} pass · {} fail", self.passed, self.listed, self.failed)
        } else if self.passed == 0 && self.skipped == 0 {
            "not run".to_string()
        } else {
            format!("{}/{} pass", self.passed, self.listed)
        }
    }
}

fn is_name_char(c: char) -> bool {
    !c.is_whitespace()
}

/// The names and outcomes found in the output of a test run (any of the supported runners).
pub fn parse_output(text: &str) -> BTreeMap<String, Outcome> {
    let mut out = BTreeMap::new();
    let t = text.trim_start();
    if t.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(t) {
            parse_jest_json(&v, &mut out);
            if !out.is_empty() {
                return out;
            }
        }
    }
    for raw in text.lines() {
        let line = raw.trim();
        // cargo test: `test a::b::c ... ok`
        if let Some(rest) = line.strip_prefix("test ") {
            if let Some((name, res)) = rest.split_once(" ... ") {
                let o = if res.starts_with("ok") { Some(Outcome::Pass) } else if res.starts_with("FAILED") { Some(Outcome::Fail) } else if res.starts_with("ignored") { Some(Outcome::Skip) } else { None };
                if let (Some(o), true) = (o, name.chars().all(|c| is_name_char(c) || c == ' ')) {
                    out.insert(name.trim().to_string(), o);
                    continue;
                }
            }
        }
        // go test -v: `--- PASS: TestX (0.00s)`
        if let Some(rest) = line.strip_prefix("--- ") {
            if let Some((word, tail)) = rest.split_once(": ") {
                let o = match word { "PASS" => Some(Outcome::Pass), "FAIL" => Some(Outcome::Fail), "SKIP" => Some(Outcome::Skip), _ => None };
                if let (Some(o), Some(name)) = (o, tail.split_whitespace().next()) {
                    out.insert(name.to_string(), o);
                    continue;
                }
            }
        }
        // pytest -v: `tests/test_a.py::test_b PASSED  [ 50%]`; -rA summary: `PASSED tests/test_a.py::test_b`
        if line.contains("::") {
            let mut words = line.split_whitespace();
            let first = words.next().unwrap_or("");
            let pyword = |w: &str| match w { "PASSED" => Some(Outcome::Pass), "FAILED" | "ERROR" => Some(Outcome::Fail), "SKIPPED" | "XFAIL" | "XPASS" => Some(Outcome::Skip), _ => None };
            if let Some(o) = pyword(first) {
                if let Some(name) = words.next().filter(|n| n.contains("::")) {
                    out.insert(name.to_string(), o);
                    continue;
                }
            } else if first.contains("::") {
                if let Some(o) = words.next().and_then(pyword) {
                    out.insert(first.to_string(), o);
                    continue;
                }
            }
        }
        // jest / vitest default output: `✓ name (3 ms)`, `✕ name`, `○ skipped name`
        let mut chars = line.chars();
        match chars.next() {
            Some('✓') | Some('√') => {
                let name = strip_timing(chars.as_str().trim());
                if !name.is_empty() {
                    out.insert(name, Outcome::Pass);
                }
            }
            Some('✕') | Some('×') | Some('✗') => {
                let name = strip_timing(chars.as_str().trim());
                if !name.is_empty() {
                    out.insert(name, Outcome::Fail);
                }
            }
            Some('○') => {
                let name = strip_timing(chars.as_str().trim().trim_start_matches("skipped").trim());
                if !name.is_empty() {
                    out.insert(name, Outcome::Skip);
                }
            }
            _ => {}
        }
    }
    out
}

/// `name (3 ms)` and `name 3ms` lose their timing.
fn strip_timing(s: &str) -> String {
    let s = s.trim();
    if let Some(i) = s.rfind(" (") {
        if s.ends_with("ms)") || s.ends_with("s)") {
            return s[..i].trim().to_string();
        }
    }
    let mut parts: Vec<&str> = s.split_whitespace().collect();
    if parts.last().is_some_and(|w| w.ends_with("ms") && w[..w.len() - 2].chars().all(|c| c.is_ascii_digit() || c == '.')) && parts.len() > 1 {
        parts.pop();
    }
    parts.join(" ")
}

fn parse_jest_json(v: &Value, out: &mut BTreeMap<String, Outcome>) {
    let Some(files) = v.get("testResults").and_then(Value::as_array) else { return };
    for f in files {
        for a in f.get("assertionResults").and_then(Value::as_array).into_iter().flatten() {
            let name = a.get("fullName").and_then(Value::as_str).or_else(|| a.get("title").and_then(Value::as_str)).unwrap_or("");
            let o = match a.get("status").and_then(Value::as_str) {
                Some("passed") => Outcome::Pass,
                Some("failed") => Outcome::Fail,
                _ => Outcome::Skip,
            };
            if !name.is_empty() {
                out.insert(name.to_string(), o);
            }
        }
    }
}

/// JUnit XML: every `<testcase classname=".." name="..">`, failed when it has a `<failure>` or `<error>`, skipped with `<skipped>`.
pub fn parse_junit(xml: &str) -> BTreeMap<String, Outcome> {
    let mut out = BTreeMap::new();
    let attr = |tag: &str, key: &str| -> Option<String> {
        let k = format!(" {key}=\"");
        let i = tag.find(&k)? + k.len();
        let j = tag[i..].find('"')?;
        Some(tag[i..i + j].replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\""))
    };
    let mut rest = xml;
    while let Some(i) = rest.find("<testcase") {
        let after = &rest[i..];
        let Some(end) = after.find('>') else { break };
        let tag = &after[..end];
        let self_closing = tag.ends_with('/');
        let body_end = if self_closing { end } else { after.find("</testcase>").unwrap_or(after.len()) };
        let body = &after[end..body_end.max(end)];
        let name = attr(tag, "name").unwrap_or_default();
        let full = match attr(tag, "classname").filter(|c| !c.is_empty()) {
            Some(c) => format!("{c}::{name}"),
            None => name.clone(),
        };
        let o = if body.contains("<failure") || body.contains("<error") { Outcome::Fail } else if body.contains("<skipped") { Outcome::Skip } else { Outcome::Pass };
        if !name.is_empty() {
            out.insert(full, o);
        }
        rest = &after[body_end.min(after.len())..];
        if rest.is_empty() {
            break;
        }
        if !rest.starts_with('<') {
            rest = &rest[1.min(rest.len())..];
        }
    }
    out
}

/// JUnit files at the conventional places of a project, if any.
pub fn read_junit(root: &Path) -> BTreeMap<String, Outcome> {
    let mut out = BTreeMap::new();
    for p in ["junit.xml", "test-results/junit.xml", "reports/junit.xml", "target/nextest/ci/junit.xml", "build/test-results/test/TEST-results.xml"] {
        if let Ok(x) = std::fs::read_to_string(root.join(p)) {
            out.extend(parse_junit(&x));
        }
    }
    out
}

fn path(root: &Path) -> std::path::PathBuf {
    root.join(".iyakki/test-results.json")
}

pub fn save(root: &Path, r: &Results) {
    let tests: serde_json::Map<String, Value> = r.tests.iter().map(|(k, v)| (k.clone(), json!(v.word()))).collect();
    if let Some(d) = path(root).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(path(root), json!({"ran_at": r.ran_at, "tests": tests}).to_string());
}

pub fn load(root: &Path) -> Option<Results> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path(root)).ok()?).ok()?;
    let tests = v.get("tests")?.as_object()?.iter().filter_map(|(k, x)| Some((k.clone(), Outcome::from_word(x.as_str()?)?))).collect();
    Some(Results { ran_at: v.get("ran_at").and_then(Value::as_i64).unwrap_or(0), tests })
}

/// Parse a finished run (its output, plus JUnit files when present), store it for the workspace, and return it. A run that
/// printed no recognisable test results stores nothing (the previous results stay).
pub fn record_run(root: &Path, stdout: &str, stderr: &str, now: i64) -> Option<Results> {
    let mut tests = parse_output(stdout);
    tests.extend(parse_output(stderr));
    for (k, v) in read_junit(root) {
        tests.entry(k).or_insert(v);
    }
    if tests.is_empty() {
        return None;
    }
    let r = Results { ran_at: now, tests };
    save(root, &r);
    Some(r)
}

fn boundary_suffix(name: &str, suffix: &str) -> bool {
    name.len() > suffix.len() && name.ends_with(suffix) && matches!(name[..name.len() - suffix.len()].chars().last(), Some(':' | '/' | '.' | ' ' | '>' | '#'))
}

/// The results entry for a listed test name: exact, then one ending the other at a path boundary, then containing.
pub fn find<'a>(listed: &str, res: &'a Results) -> Option<(&'a String, Outcome)> {
    let l = listed.trim();
    if l.is_empty() {
        return None;
    }
    if let Some((k, v)) = res.tests.get_key_value(l) {
        return Some((k, *v));
    }
    if let Some((k, v)) = res.tests.iter().find(|(k, _)| boundary_suffix(k, l) || boundary_suffix(l, k)) {
        return Some((k, *v));
    }
    // a listed `file::test` whose test part is a result of its own
    if let Some(tail) = l.rsplit("::").next().filter(|t| *t != l) {
        if let Some((k, v)) = res.tests.iter().find(|(k, _)| *k == tail || boundary_suffix(k, tail)) {
            return Some((k, *v));
        }
    }
    res.tests.iter().find(|(k, _)| k.contains(l) || (l.len() > 8 && l.contains(k.as_str()) && k.len() > 8)).map(|(k, v)| (k, *v))
}

/// What the results say about the tests a feature lists.
pub fn match_row(listed: &[String], res: &Results) -> RowTests {
    let mut r = RowTests { listed: listed.len(), ..Default::default() };
    for l in listed {
        match find(l, res).map(|x| x.1) {
            Some(Outcome::Pass) => r.passed += 1,
            Some(Outcome::Fail) => r.failed += 1,
            Some(Outcome::Skip) => r.skipped += 1,
            None => r.not_run += 1,
        }
    }
    r
}

/// Tests of the run that no feature lists.
pub fn unlinked(all_listed: &[String], res: &Results) -> Vec<String> {
    let mut linked: std::collections::BTreeSet<String> = Default::default();
    for l in all_listed {
        if let Some((k, _)) = find(l, res) {
            linked.insert(k.clone());
        }
    }
    res.tests.keys().filter(|k| !linked.contains(*k)).cloned().collect()
}

/// Seconds ago as "3 min ago" / "2 h ago".
pub fn ago(now: i64, then: i64) -> String {
    let s = (now - then).max(0);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARGO: &str = "running 4 tests\ntest scope::tests::resolves ... ok\ntest scope::tests::explains ... ok\ntest agent::tests::slow ... FAILED\ntest agent::tests::later ... ignored, needs a server\n\ntest result: FAILED. 2 passed; 1 failed; 1 ignored\n";
    const PYTEST: &str = "tests/test_shop.py::test_cart PASSED                [ 33%]\ntests/test_shop.py::test_checkout FAILED            [ 66%]\ntests/test_shop.py::test_tax SKIPPED (no db)        [100%]\n=========== short test summary ===========\nPASSED tests/test_shop.py::test_cart\n";
    const GO: &str = "=== RUN   TestAdd\n--- PASS: TestAdd (0.00s)\n=== RUN   TestSub\n--- FAIL: TestSub (0.01s)\n    --- SKIP: TestSub/edge (0.00s)\nFAIL\n";
    const JEST: &str = " PASS  src/cart.test.js\n  cart\n    ✓ adds an item (3 ms)\n    ✕ removes an item (2 ms)\n    ○ skipped applies a coupon\n";
    const JEST_JSON: &str = r#"{"numTotalTests":2,"testResults":[{"name":"/p/a.test.js","assertionResults":[{"fullName":"cart adds an item","status":"passed"},{"fullName":"cart removes","status":"failed"},{"title":"later","status":"pending"}]}]}"#;
    const JUNIT: &str = r#"<testsuites><testsuite name="s"><testcase classname="app.Cart" name="adds" time="0.1"/><testcase classname="app.Cart" name="removes"><failure message="boom">x</failure></testcase><testcase classname="app.Tax" name="later"><skipped/></testcase></testsuite></testsuites>"#;

    #[test]
    fn the_output_of_every_common_runner_becomes_names_and_outcomes() {
        let c = parse_output(CARGO);
        assert_eq!((c["scope::tests::resolves"], c["agent::tests::slow"], c["agent::tests::later"]), (Outcome::Pass, Outcome::Fail, Outcome::Skip));
        assert_eq!(c.len(), 4);
        let p = parse_output(PYTEST);
        assert_eq!((p["tests/test_shop.py::test_cart"], p["tests/test_shop.py::test_checkout"], p["tests/test_shop.py::test_tax"]), (Outcome::Pass, Outcome::Fail, Outcome::Skip));
        let g = parse_output(GO);
        assert_eq!((g["TestAdd"], g["TestSub"], g["TestSub/edge"]), (Outcome::Pass, Outcome::Fail, Outcome::Skip));
        let j = parse_output(JEST);
        assert_eq!((j["adds an item"], j["removes an item"], j["applies a coupon"]), (Outcome::Pass, Outcome::Fail, Outcome::Skip));
        let jj = parse_output(JEST_JSON);
        assert_eq!((jj["cart adds an item"], jj["cart removes"], jj["later"]), (Outcome::Pass, Outcome::Fail, Outcome::Skip));
        let x = parse_junit(JUNIT);
        assert_eq!((x["app.Cart::adds"], x["app.Cart::removes"], x["app.Tax::later"]), (Outcome::Pass, Outcome::Fail, Outcome::Skip));
        assert!(parse_output("compiling…\nnothing here\n").is_empty());
    }

    fn res(pairs: &[(&str, Outcome)]) -> Results {
        Results { ran_at: 100, tests: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect() }
    }

    #[test]
    fn listed_tests_are_found_by_exact_name_then_suffix_then_containment() {
        let r = res(&[("scope::tests::resolves", Outcome::Pass), ("agent::tests::slow", Outcome::Fail), ("tests/test_shop.py::test_cart", Outcome::Pass), ("TestAdd", Outcome::Pass)]);
        let listed: Vec<String> = ["scope::tests::resolves", "iyakki-agent::agent::tests::slow", "test_cart", "test_shop.py::test_cart", "TestAdd", "never_ran"].map(String::from).to_vec();
        let m = match_row(&listed, &r);
        assert_eq!((m.listed, m.passed, m.failed, m.not_run), (6, 4, 1, 1), "{m:?}");
        assert_eq!(m.label(), "4/6 pass · 1 fail");
        assert!(!m.all_passed());
        assert!(match_row(&["TestAdd".into()], &r).all_passed());
        assert_eq!(match_row(&["nothing".into()], &r).label(), "not run");
        assert_eq!(match_row(&[], &r).label(), "");
        // a name is not matched by a mere tail inside a word
        assert_eq!(match_row(&["cart".into()], &res(&[("scart", Outcome::Pass)])).not_run, 0, "containment is the last resort");
        assert_eq!(match_row(&["resolves".into()], &res(&[("scope::tests::unresolves", Outcome::Pass)])).passed, 1, "containment, as the last resort");
        // tests no feature lists
        let un = unlinked(&listed, &r);
        assert!(un.is_empty(), "{un:?}");
        assert_eq!(unlinked(&["TestAdd".into()], &r).len(), 3);
    }

    #[test]
    fn results_are_stored_per_workspace_and_a_run_without_results_keeps_the_old_ones() {
        let d = std::env::temp_dir().join(format!("iyakki-testres-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(load(&d).is_none());
        let r = record_run(&d, CARGO, "", 1234).unwrap();
        assert_eq!(r.tests.len(), 4);
        assert_eq!(load(&d).unwrap(), r);
        assert!(record_run(&d, "error: could not compile", "", 2000).is_none());
        assert_eq!(load(&d).unwrap().ran_at, 1234, "the old results stay");
        // junit at a conventional path is merged in
        std::fs::write(d.join("junit.xml"), JUNIT).unwrap();
        let r2 = record_run(&d, GO, "", 3000).unwrap();
        assert!(r2.tests.contains_key("TestAdd") && r2.tests.contains_key("app.Cart::adds"));
        assert_eq!(ago(3600 * 3, 0), "3 h ago");
        assert_eq!(ago(30, 0), "just now");
        let _ = std::fs::remove_dir_all(&d);
    }
}
