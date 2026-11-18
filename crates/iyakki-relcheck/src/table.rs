//! The small checked-in review files of the licence gate (`legal/*.txt`) are pipe-separated tables: one row per line, `#` starts a
//! comment, blank lines are ignored. Every row that waives a check carries a reason and a date (`YYYY-MM-DD`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub f: Vec<String>,
}

pub fn parse(text: &str) -> Vec<Row> {
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let t = l.trim();
            if t.is_empty() || t.starts_with('#') {
                return None;
            }
            Some(Row { line: i + 1, f: t.split('|').map(|s| s.trim().to_string()).collect() })
        })
        .collect()
}

/// `YYYY-MM-DD` with a plausible month and day.
pub fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (y, m, d) = (s[0..4].parse::<u32>(), s[5..7].parse::<u32>(), s[8..10].parse::<u32>());
    matches!((y, m, d), (Ok(y), Ok(m), Ok(d)) if y >= 2024 && (1..=12).contains(&m) && (1..=31).contains(&d))
}

/// A reason worth reading: not a placeholder.
pub fn valid_reason(s: &str) -> bool {
    s.chars().count() >= 12 && !matches!(s.to_lowercase().as_str(), "todo" | "tbd" | "n/a" | "none")
}

/// Shell-style match where `*` matches any run of characters (including `/`) and `?` one character.
pub fn glob(pat: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_dates_and_globs() {
        let r = parse("# c\n\na | b |c\n  x|y\n");
        assert_eq!((r.len(), r[0].f.clone(), r[1].line), (2, vec!["a".to_string(), "b".into(), "c".into()], 4));
        assert!(valid_date("2026-10-26") && !valid_date("26-10-2026") && !valid_date("2026-13-01") && !valid_date("soon"));
        assert!(valid_reason("reviewed against the LICENSE file") && !valid_reason("TODO") && !valid_reason("ok"));
        assert!(glob("lib/firmware/*", "lib/firmware/brcm/x.bin") && glob("*.apple,*", "lib/firmware/brcm/a.apple,j140k.bin") && !glob("usr/bin/?", "usr/bin/ab") && glob("a*b*c", "aXbYc") && !glob("a*b", "abc"));
    }
}
