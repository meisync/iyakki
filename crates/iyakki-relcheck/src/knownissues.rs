//! KNOWN-ISSUES.md structure check (owner goal: the list goes to zero, honestly, with a way to see progress).
//!
//! New entry form, one line: `* KI-nnn [CLASS] statement | Closes by: thing | Row: F-... or none`, CLASS one of BUG, UNPROVEN, LIMIT.
//! FIXED entries are deleted from the list and recorded with their evidence in `docs/development/KNOWN-ISSUES-FIXED.md`.
//!
//! Transitional rule: an entry without a CLASS counts as UNTRIAGED. UNTRIAGED may not exceed the number in
//! `docs/development/known-issues-ratchet.txt` (`untriaged N`, only ever lowered by hand when entries are sorted; the check only refuses a higher count), and after the file's `until YYYY-MM-DD` date it must be zero.

use std::collections::{BTreeMap, BTreeSet};

pub const CLASSES: [&str; 3] = ["BUG", "UNPROVEN", "LIMIT"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub line: usize,
    /// `Some(class)` when a `[CLASS]` tag follows the id (any word; validated by `check`), `None` for an untriaged entry.
    pub class: Option<String>,
    pub closes_by: Option<String>,
    pub row: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counts {
    pub bug: usize,
    pub unproven: usize,
    pub limit: usize,
    pub untriaged: usize,
}

impl Counts {
    pub fn total(&self) -> usize {
        self.bug + self.unproven + self.limit + self.untriaged
    }
    pub fn line(&self) -> String {
        format!("BUG {}, UNPROVEN {}, LIMIT {}, UNTRIAGED {}", self.bug, self.unproven, self.limit, self.untriaged)
    }
}

fn is_ki(s: &str) -> bool {
    s.len() >= 6 && s.starts_with("KI-") && s[3..].bytes().all(|b| b.is_ascii_digit())
}

/// Every `KI-` followed by at least three digits, once each (shorter numbers are prose such as "KI-1").
pub fn cited_ids(text: &str) -> BTreeSet<String> {
    let b = text.as_bytes();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i + 3 < b.len() {
        if &b[i..i + 3] == b"KI-" && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let mut j = i + 3;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j - (i + 3) >= 3 {
                out.insert(text[i..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// The entries of the list: bullet lines `* KI-nnn ...` above the generated appendix, with their indented continuation lines.
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut bodies: Vec<String> = Vec::new();
    for (n, l) in text.lines().enumerate() {
        if l.starts_with("## Appendix") || l.contains("<!-- appendix:start -->") {
            break;
        }
        if let Some(rest) = l.strip_prefix("* KI-") {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                continue;
            }
            let after = rest[digits.len()..].trim_start();
            let class = after.strip_prefix('[').and_then(|a| a.split_once(']')).map(|(c, _)| c.trim().to_string());
            entries.push(Entry { id: format!("KI-{digits}"), line: n + 1, class, closes_by: None, row: None });
            bodies.push(l.to_string());
        } else if (l.starts_with("  ") || l.is_empty()) && !bodies.is_empty() && !l.is_empty() {
            let last = bodies.len() - 1;
            bodies[last].push(' ');
            bodies[last].push_str(l.trim());
        }
    }
    for (e, body) in entries.iter_mut().zip(bodies) {
        let field = |name: &str| {
            body.rsplit_once(&format!("| {name}:")).map(|(_, v)| v.split(" | ").next().unwrap_or("").trim().to_string()).filter(|v| !v.is_empty())
        };
        e.closes_by = field("Closes by");
        // `Row:` is `none` or one or more F- ids separated by commas or spaces; whatever prose follows the ids is not part of it
        e.row = field("Row").map(|r| {
            let mut ids: Vec<&str> = Vec::new();
            for t in r.split(|c: char| c == ',' || c == ' ' || c == ';').map(|t| t.trim_matches(|c: char| c == '`' || c == '.' || c == '(' || c == ')')).filter(|t| !t.is_empty()) {
                if t == "none" && ids.is_empty() {
                    return "none".to_string();
                }
                if t.starts_with("F-") && t.len() > 2 && t.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-') {
                    ids.push(t);
                } else {
                    break;
                }
            }
            ids.join(" ")
        }).filter(|r| !r.is_empty());
    }
    entries
}

pub fn counts(entries: &[Entry]) -> Counts {
    let mut c = Counts::default();
    for e in entries {
        match e.class.as_deref() {
            Some("BUG") => c.bug += 1,
            Some("UNPROVEN") => c.unproven += 1,
            Some("LIMIT") => c.limit += 1,
            _ => c.untriaged += 1,
        }
    }
    c
}

/// The ids of a fixed registry (`* KI-nnn | date | evidence` lines) with their line.
pub fn parse_fixed(text: &str) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for (n, l) in text.lines().enumerate() {
        if let Some(rest) = l.strip_prefix("* ") {
            let id = rest.split(|c: char| c == ' ' || c == '|').next().unwrap_or("");
            if is_ki(id) {
                m.insert(id.to_string(), n + 1);
            }
        }
    }
    m
}

/// Ratchet file: `untriaged N`, `dangling N` (distinct cited ids that are in neither the list nor the registry) and `until YYYY-MM-DD`;
/// `#` lines are comments.
pub fn parse_ratchet(text: &str) -> Option<(usize, usize, String)> {
    let mut n = None;
    let mut d = None;
    let mut until = None;
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        match l.split_whitespace().collect::<Vec<_>>().as_slice() {
            ["untriaged", v] => n = v.parse().ok(),
            ["dangling", v] => d = v.parse().ok(),
            ["until", d] => until = Some(d.to_string()),
            _ => {}
        }
    }
    Some((n?, d?, until?))
}

pub struct Inputs<'a> {
    pub known_issues: &'a str,
    pub features: &'a str,
    pub fixed: &'a str,
    pub ratchet: &'a str,
    /// (path, text) of FEATURES.md-adjacent documents whose `KI-nnn` citations must resolve.
    pub cited_in: Vec<(String, &'a str)>,
    /// `YYYY-MM-DD`, injected so the check is deterministic in tests.
    pub today: &'a str,
}

pub struct Report {
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
    pub dangling: usize,
    pub counts: Counts,
    pub fixed: usize,
}

fn feature_ids(features: &str) -> BTreeSet<String> {
    features.lines().filter_map(|l| l.strip_prefix("| ")).filter_map(|r| r.split('|').next()).map(str::trim).filter(|id| id.starts_with("F-")).map(String::from).collect()
}

pub fn check(i: &Inputs) -> Report {
    let mut p: Vec<String> = Vec::new();
    let entries = parse(i.known_issues);
    let c = counts(&entries);
    let rows = feature_ids(i.features);
    let fixed = parse_fixed(i.fixed);
    let mut w: Vec<String> = Vec::new();
    let until_date = parse_ratchet(i.ratchet).map(|r| r.2).unwrap_or_default();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for e in &entries {
        if let Some(first) = seen.insert(&e.id, e.line) {
            p.push(format!("KNOWN-ISSUES.md:{}: {} is already used at line {first}", e.line, e.id));
        }
        if fixed.contains_key(&e.id) {
            // transitional: the agents sorting the sections delete these; a problem once the ratchet date has passed
            let msg = format!("KNOWN-ISSUES.md:{}: {} is recorded as fixed in KNOWN-ISSUES-FIXED.md; delete it from the list", e.line, e.id);
            if i.today > until_date.as_str() { p.push(msg) } else { w.push(msg) }
        }
        match e.class.as_deref() {
            None => continue,
            Some("FIXED") => p.push(format!("KNOWN-ISSUES.md:{}: {} is FIXED: delete it and record it in KNOWN-ISSUES-FIXED.md", e.line, e.id)),
            Some(k) if !CLASSES.contains(&k) => p.push(format!("KNOWN-ISSUES.md:{}: {} has class [{k}]; use BUG, UNPROVEN or LIMIT", e.line, e.id)),
            Some(_) => {}
        }
        if e.closes_by.is_none() {
            p.push(format!("KNOWN-ISSUES.md:{}: {} has no `| Closes by: ...` field", e.line, e.id));
        }
        match e.row.as_deref() {
            None => p.push(format!("KNOWN-ISSUES.md:{}: {} has no `| Row: F-... or none` field", e.line, e.id)),
            Some("none") => {}
            Some(r) => {
                for id in r.split(|c: char| c == ',' || c == ' ').filter(|s| !s.is_empty()) {
                    if !rows.contains(id) {
                        p.push(format!("KNOWN-ISSUES.md:{}: {} names row {id}, which is not in FEATURES.md", e.line, e.id));
                    }
                }
            }
        }
    }
    // citations
    let known: BTreeSet<&str> = entries.iter().map(|e| e.id.as_str()).chain(fixed.keys().map(String::as_str)).collect();
    let mut docs: Vec<(String, &str)> = vec![("FEATURES.md".to_string(), i.features)];
    docs.extend(i.cited_in.iter().map(|(a, b)| (a.clone(), *b)));
    let mut dangling: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, text) in &docs {
        for id in cited_ids(text) {
            if !known.contains(id.as_str()) {
                dangling.entry(id).or_default().push(path.clone());
            }
        }
    }
    // ratchet
    match parse_ratchet(i.ratchet) {
        None => p.push("known-issues-ratchet.txt: needs `untriaged N` and `until YYYY-MM-DD`".into()),
        Some((max, max_dangling, until)) => {
            if dangling.len() > max_dangling {
                for (id, at) in &dangling {
                    p.push(format!("{}: cites {id}, which is neither in KNOWN-ISSUES.md nor in KNOWN-ISSUES-FIXED.md", at.join(", ")));
                }
                p.push(format!("{} cited ids have no entry and no fixed record, the ratchet allows {max_dangling}: add the entry or the fixed record", dangling.len()));
            }
            if i.today > until.as_str() && !dangling.is_empty() {
                p.push(format!("the ratchet date {until} has passed and {} cited ids still have no entry", dangling.len()));
            }
            if c.untriaged > max {
                p.push(format!("UNTRIAGED is {}, the ratchet allows {max}: give the new entries a class (or lower nothing, raise nothing)", c.untriaged));
            }
            if i.today > until.as_str() && c.untriaged > 0 {
                p.push(format!("the ratchet date {until} has passed and {} entries are still UNTRIAGED", c.untriaged));
            }
        }
    }
    Report { problems: p, warnings: w, dangling: dangling.len(), counts: c, fixed: fixed.len() }
}

/// Today as `YYYY-MM-DD` (UTC), without a date crate.
pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let z = secs / 86400 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Read the repository files and check them.
pub fn check_repo(root: &std::path::Path) -> Report {
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_default();
    let ki = read("KNOWN-ISSUES.md");
    let features = read("FEATURES.md");
    let fixed = read("docs/development/KNOWN-ISSUES-FIXED.md");
    let ratchet = read("docs/development/known-issues-ratchet.txt");
    let mut files = Vec::new();
    collect_md(&root.join("docs"), &mut files);
    for f in ["README.md", "CHANGELOG.md", "SECURITY.md", "SUPPORT.md"] {
        files.push(root.join(f));
    }
    let skip = ["KNOWN-ISSUES-FIXED.md", "KNOWN-ISSUES-BURNDOWN.md"];
    let texts: Vec<(String, String)> = files
        .iter()
        .filter(|f| !skip.iter().any(|s| f.ends_with(s)))
        .filter_map(|f| Some((f.strip_prefix(root).unwrap_or(f).display().to_string(), std::fs::read_to_string(f).ok()?)))
        .collect();
    check(&Inputs { known_issues: &ki, features: &features, fixed: &fixed, ratchet: &ratchet, cited_in: texts.iter().map(|(a, b)| (a.clone(), b.as_str())).collect(), today: &today() })
}

fn collect_md(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_md(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEAT: &str = "| F-AAA-01 | x |\n| F-AAA-02 | y |\n";
    const GOOD: &str = "# KI\n* KI-001 [BUG] a thing | Closes by: a fix | Row: F-AAA-01\n* KI-002 [LIMIT] hw | Closes by: never | Row: none\n* KI-003 [UNPROVEN] x | Closes by: a test on a Mac | Row: F-AAA-01, F-AAA-02\n* KI-004 old style entry\n  continued\n## Appendix: every row\n* KI-999 [BUG] ignored\n";

    fn run(ki: &str, fixed: &str, ratchet: &str, cite: &str, today: &str) -> Report {
        check(&Inputs { known_issues: ki, features: FEAT, fixed, ratchet, cited_in: vec![("docs/a.md".into(), cite)], today })
    }

    #[test]
    fn counts_and_clean_pass() {
        let r = run(GOOD, "* KI-010 | 2026-11-19 | test\n", "untriaged 1\ndangling 0\nuntil 2026-12-11\n", "see KI-001 and KI-010, not KI-1", "2026-11-19");
        assert_eq!(r.problems, Vec::<String>::new());
        assert_eq!(r.counts, Counts { bug: 1, unproven: 1, limit: 1, untriaged: 1 });
        assert_eq!(r.fixed, 1);
    }

    #[test]
    fn each_rule_fires() {
        let ki = "* KI-001 [BUG] a | Closes by: x | Row: F-ZZZ-01\n* KI-001 [LIMIT] dup | Closes by: x | Row: none\n* KI-002 [WRONG] c | Closes by: x | Row: none\n* KI-003 [BUG] no fields\n* KI-004 [FIXED] gone | Closes by: x | Row: none\n";
        let r = run(ki, "* KI-004 | d | e\n", "untriaged 0\ndangling 0\nuntil 2026-12-11\n", "KI-777", "2026-11-19");
        let all = r.problems.iter().chain(&r.warnings).cloned().collect::<Vec<_>>().join("\n");
        for want in ["already used", "F-ZZZ-01", "class [WRONG]", "no `| Closes by", "no `| Row", "is FIXED", "recorded as fixed", "cites KI-777", "ratchet allows 0"] {
            assert!(all.contains(want), "missing {want} in {all}");
        }
    }

    #[test]
    fn ratchet_only_goes_down_and_date_forces_zero() {
        let ki = "* KI-001 old\n* KI-002 old\n";
        assert!(run(ki, "", "untriaged 1\ndangling 0\nuntil 2026-12-11\n", "", "2026-11-19").problems.iter().any(|p| p.contains("allows 1")));
        assert!(run(ki, "", "untriaged 3\ndangling 0\nuntil 2026-12-11\n", "", "2026-11-19").problems.is_empty());
        assert!(run(ki, "", "untriaged 2\ndangling 0\nuntil 2026-12-11\n", "", "2026-11-19").problems.is_empty());
        assert!(run(ki, "", "untriaged 2\ndangling 0\nuntil 2026-12-11\n", "", "2026-12-12").problems.iter().any(|p| p.contains("has passed")));
        assert!(run(ki, "", "", "", "2026-11-19").problems.iter().any(|p| p.contains("ratchet.txt")));
    }

    #[test]
    fn row_field_ignores_prose_after_the_ids() {
        let e = parse("* KI-001 [BUG] a | Closes by: x | Row: F-AAA-01, F-AAA-02 and more text\n* KI-002 [BUG] b | Closes by: x | Row: none on disk: GPT\n");
        assert_eq!(e[0].row.as_deref(), Some("F-AAA-01 F-AAA-02"));
        assert_eq!(e[1].row.as_deref(), Some("none"));
    }

    #[test]
    fn dates_and_ids() {
        assert_eq!(today().len(), 10);
        assert_eq!(cited_ids("KI-077, KI-1010 and XKI-555 KI-12").into_iter().collect::<Vec<_>>(), ["KI-077", "KI-1010"]);
    }
}
