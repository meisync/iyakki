//! FEATURES.md totals: the summary line must match the rows.

use std::collections::BTreeMap;

const STATUSES: [&str; 5] = ["done", "partial", "planned", "blocked", "deprecated"];

/// Count the rows by status.
pub fn count(text: &str) -> BTreeMap<String, usize> {
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for line in text.lines().filter(|l| l.starts_with("| F-")) {
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if let Some(s) = cells.get(3) {
            *m.entry((*s).to_string()).or_default() += 1;
        }
    }
    m
}

/// The expected summary text: `done 219 · partial 121 · ... · total 397`.
pub fn summary(text: &str) -> String {
    let c = count(text);
    let mut parts: Vec<String> = STATUSES.iter().filter_map(|s| c.get(*s).map(|n| format!("{s} {n}"))).collect();
    parts.push(format!("total {}", c.values().sum::<usize>()));
    parts.join(" · ")
}

/// Problems: a missing or wrong summary line, an unknown status.
pub fn check(text: &str) -> Vec<String> {
    let mut bad = Vec::new();
    let c = count(text);
    for s in c.keys().filter(|s| !STATUSES.contains(&s.as_str())) {
        bad.push(format!("unknown status {s:?}"));
    }
    let (Some(a), Some(b)) = (text.find("<!-- summary:start -->"), text.find("<!-- summary:end -->")) else {
        bad.push("the summary markers are missing".into());
        return bad;
    };
    let have = text[a + "<!-- summary:start -->".len()..b].trim();
    let want = summary(text);
    if have != want {
        bad.push(format!("the totals line says \"{have}\" but the rows give \"{want}\""));
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "x <!-- summary:start -->done 2 · partial 1 · total 3<!-- summary:end -->\n| F-A-01 | a | P1 | done | P1 | 100% | t | |\n| F-A-02 | b | P1 | done | P1 | 100% | t | |\n| F-A-03 | c | P1 | partial | P1 | 50% | | |\n";

    #[test]
    fn totals_are_counted_and_compared() {
        assert_eq!(summary(T), "done 2 · partial 1 · total 3");
        assert!(check(T).is_empty());
        assert!(check(&T.replace("done 2", "done 3"))[0].contains("totals line"));
        assert!(check("nothing")[0].contains("markers"));
        assert!(check(&T.replace("| partial |", "| wip |"))[0].contains("unknown status"));
    }
}
