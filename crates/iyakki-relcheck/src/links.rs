//! A small markdown link checker: relative links must point at something that exists, and `#anchors` must match a heading of
//! the file they point into (GitHub's way of making ids). Web links are not fetched. Code blocks and code spans are skipped.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub struct Problem {
    pub file: PathBuf,
    pub line: usize,
    pub target: String,
    pub why: String,
}

/// GitHub's heading id: lower case, markdown marks dropped, spaces become `-`, punctuation (except `-` and `_`) dropped.
pub fn slug(heading: &str) -> String {
    let mut text = String::new();
    // [text](url) -> text
    let mut rest = heading;
    while let Some(i) = rest.find('[') {
        text.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match (after.find("]("), after.find(')')) {
            (Some(a), Some(b)) if a < b => {
                text.push_str(&after[..a]);
                rest = &after[b + 1..];
            }
            _ => {
                text.push('[');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    let mut out = String::new();
    for c in text.chars() {
        if c == '`' || c == '*' {
            continue;
        }
        if c == ' ' {
            out.push('-');
        } else if c == '-' || c == '_' || c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The anchors a markdown text offers (duplicates get `-1`, `-2`, ...). Headings inside code fences do not count.
pub fn anchors(md: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let mut fence = false;
    for line in md.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence || !line.starts_with('#') {
            continue;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if hashes > 6 || !line[hashes..].starts_with(' ') {
            continue;
        }
        let base = slug(line[hashes..].trim().trim_end_matches('#').trim());
        let mut id = base.clone();
        let mut n = 0;
        while set.contains(&id) {
            n += 1;
            id = format!("{base}-{n}");
        }
        set.insert(id);
    }
    set
}

/// `(line number, target)` of every link in a markdown text: `[x](target)`, `![x](target)` and `[x]: target` definitions.
pub fn links(md: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut fence = false;
    for (n, line) in md.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        // drop inline code spans
        let mut clean = String::new();
        let mut inside = false;
        for c in line.chars() {
            if c == '`' {
                inside = !inside;
                continue;
            }
            if !inside {
                clean.push(c);
            }
        }
        let mut rest = clean.as_str();
        while let Some(i) = rest.find("](") {
            let after = &rest[i + 2..];
            // the target ends at the matching ')' (parentheses inside a target are balanced)
            let mut depth = 0;
            let mut end = None;
            for (k, c) in after.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' if depth == 0 => {
                        end = Some(k);
                        break;
                    }
                    ')' => depth -= 1,
                    _ => {}
                }
            }
            let Some(end) = end else { break };
            let raw = after[..end].trim();
            // `target "title"`
            let target = if let Some(stripped) = raw.strip_prefix('<') { stripped.split('>').next().unwrap_or("") } else { raw.split(' ').next().unwrap_or("") };
            if !target.is_empty() {
                out.push((n + 1, target.to_string()));
            }
            rest = &after[end + 1..];
        }
        let d = clean.trim_start();
        if d.starts_with('[') {
            if let Some(i) = d.find("]:") {
                let t = d[i + 2..].trim().split(' ').next().unwrap_or("");
                if !t.is_empty() {
                    out.push((n + 1, t.to_string()));
                }
            }
        }
    }
    out
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() && s.is_char_boundary(i + 1) && s.is_char_boundary(i + 3) {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_external(t: &str) -> bool {
    t.contains("://") || t.starts_with("mailto:") || t.starts_with("tel:") || t.starts_with("data:") || t.starts_with('{') || t.contains("{{")
}

/// Check one markdown file; `read` gives the text of another file (for anchors) and says whether a path exists.
pub fn check_file(root: &Path, file: &Path, text: &str, exists: &dyn Fn(&Path) -> bool, read: &dyn Fn(&Path) -> Option<String>) -> Vec<Problem> {
    let mut problems = Vec::new();
    let dir = file.parent().unwrap_or(Path::new("."));
    let own = anchors(text);
    for (line, target) in links(text) {
        if is_external(&target) {
            continue;
        }
        let (path_part, anchor) = match target.split_once('#') {
            Some((p, a)) => (p.to_string(), Some(a.to_string())),
            None => (target.clone(), None),
        };
        let path_part = decode(path_part.split('?').next().unwrap_or(""));
        let mut bad = |why: String| problems.push(Problem { file: file.to_path_buf(), line, target: target.clone(), why });
        if path_part.is_empty() {
            if let Some(a) = anchor {
                if !a.is_empty() && !own.contains(&decode(&a).to_lowercase()) {
                    bad(format!("no heading `#{a}` in this file"));
                }
            }
            continue;
        }
        let dest = if let Some(abs) = path_part.strip_prefix('/') { root.join(abs) } else { dir.join(&path_part) };
        if !exists(&dest) {
            bad(format!("{} does not exist", dest.display()));
            continue;
        }
        if let Some(a) = anchor.filter(|a| !a.is_empty()) {
            if dest.extension().is_some_and(|e| e == "md") {
                match read(&dest) {
                    Some(t) if anchors(&t).contains(&decode(&a).to_lowercase()) => {}
                    Some(_) => bad(format!("no heading `#{a}` in {}", dest.display())),
                    None => {}
                }
            }
        }
    }
    problems
}

/// Check README.md, the other top-level markdown files and everything under docs/ in `root`.
pub fn check_repo(root: &Path) -> Vec<Problem> {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in [root.to_path_buf(), root.join("docs")] {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_file() && p.extension().is_some_and(|x| x == "md") {
                files.push(p);
            }
        }
    }
    files.sort();
    let exists = |p: &Path| p.exists();
    let read = |p: &Path| std::fs::read_to_string(p).ok();
    let mut all = Vec::new();
    for f in files {
        if let Ok(t) = std::fs::read_to_string(&f) {
            // paths in problems are shown relative to the repository
            let mut ps = check_file(root, &f, &t, &exists, &read);
            for p in &mut ps {
                p.file = p.file.strip_prefix(root).unwrap_or(&p.file).to_path_buf();
            }
            all.extend(ps);
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn slugs_follow_github() {
        assert_eq!(slug("Try it (start here)"), "try-it-start-here");
        assert_eq!(slug("What's in the `box`"), "whats-in-the-box");
        assert_eq!(slug("A [link](x.md) here"), "a-link-here");
        assert_eq!(slug("1. Intro & more"), "1-intro--more");
    }

    #[test]
    fn anchors_skip_fences_and_number_duplicates() {
        let a = anchors("# One\n## Two\n```\n# not a heading\n```\n## Two\n####### seven\n#nospace\n");
        assert!(a.contains("one") && a.contains("two") && a.contains("two-1"));
        assert!(!a.contains("not-a-heading") && a.len() == 3);
    }

    #[test]
    fn links_are_found_outside_code() {
        let md = "see [a](docs/A.md#x) and `[no](ignored.md)`\n```\n[no](fenced.md)\n```\n![img](pic.png \"t\") [b](<with space.md>) [web](https://x.y/z) [c](f(1).md)\n[ref]: other.md\n";
        let l: Vec<String> = links(md).into_iter().map(|x| x.1).collect();
        assert_eq!(l, vec!["docs/A.md#x", "pic.png", "with space.md", "https://x.y/z", "f(1).md", "other.md"]);
    }

    #[test]
    fn broken_paths_and_anchors_are_reported() {
        let files: BTreeMap<PathBuf, String> = [("/r/README.md", "# Top\n[ok](docs/A.md#sec) [bad](docs/A.md#nope) [gone](docs/B.md) [self](#top) [selfbad](#nothing) [web](http://x) [abs](/docs/A.md)\n"), ("/r/docs/A.md", "# A\n## Sec\n")]
            .into_iter()
            .map(|(k, v)| (PathBuf::from(k), v.to_string()))
            .collect();
        let exists = |p: &Path| files.contains_key(p);
        let read = |p: &Path| files.get(p).cloned();
        let p = check_file(Path::new("/r"), Path::new("/r/README.md"), &files[Path::new("/r/README.md")], &exists, &read);
        let targets: Vec<&str> = p.iter().map(|x| x.target.as_str()).collect();
        assert_eq!(targets, vec!["docs/A.md#nope", "docs/B.md", "#nothing"], "{p:?}");
        assert_eq!(p[0].line, 2);
    }

    #[test]
    fn percent_encoding_is_decoded() {
        assert_eq!(decode("a%20b.md"), "a b.md");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz"), "%zz");
    }
}
