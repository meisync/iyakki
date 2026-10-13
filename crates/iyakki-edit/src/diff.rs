//! Line diff (Myers' O(ND) algorithm) grouped into hunks that can be accepted or rejected one by one.

/// One contiguous change: `old` lines (starting at `old_start`) become `new` lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old: Vec<String>,
    pub new: Vec<String>,
}

/// Cap on edit distance explored; beyond it the whole differing middle becomes one hunk.
const MAX_D: usize = 3000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// Edit script from `a` to `b` (Myers). `None` when the files are too different to diff cheaply.
fn myers(a: &[&str], b: &[&str]) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (n + m) as usize;
    if max == 0 {
        return Some(vec![]);
    }
    let limit = max.min(MAX_D);
    let off = limit as isize + 1;
    let mut v = vec![0isize; 2 * limit + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'outer: for d in 0..=limit as isize {
        // only the diagonals that can matter at depth d are kept: memory is O(D²), not O(D·(N+M))
        trace.push(v[(off - d - 1) as usize..=(off + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[(off + k - 1) as usize] < v[(off + k + 1) as usize]) { v[(off + k + 1) as usize] } else { v[(off + k - 1) as usize] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(off + k) as usize] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
    }
    let d_end = found?;
    // walk back through the saved V arrays
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=d_end).rev() {
        let vv = &trace[d as usize];
        let at = |k: isize| vv[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                ops.push(Op::Insert);
                y -= 1;
            } else {
                ops.push(Op::Delete);
                x -= 1;
            }
        }
    }
    ops.reverse();
    Some(ops)
}

/// The changes needed to turn `old` into `new`, as hunks in order.
pub fn diff_hunks(old: &[String], new: &[String]) -> Vec<Hunk> {
    let mut pre = 0;
    while pre < old.len() && pre < new.len() && old[pre] == new[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < old.len() - pre && suf < new.len() - pre && old[old.len() - 1 - suf] == new[new.len() - 1 - suf] {
        suf += 1;
    }
    let (ao, bo): (Vec<&str>, Vec<&str>) = (old[pre..old.len() - suf].iter().map(String::as_str).collect(), new[pre..new.len() - suf].iter().map(String::as_str).collect());
    if ao.is_empty() && bo.is_empty() {
        return vec![];
    }
    let Some(ops) = myers(&ao, &bo) else {
        return vec![Hunk { old_start: pre, old: old[pre..old.len() - suf].to_vec(), new: new[pre..new.len() - suf].to_vec() }];
    };
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut cur: Option<Hunk> = None;
    for op in ops {
        match op {
            Op::Equal => {
                if let Some(h) = cur.take() {
                    hunks.push(h);
                }
                i += 1;
                j += 1;
            }
            Op::Delete => {
                cur.get_or_insert_with(|| Hunk { old_start: pre + i, old: vec![], new: vec![] }).old.push(ao[i].to_string());
                i += 1;
            }
            Op::Insert => {
                cur.get_or_insert_with(|| Hunk { old_start: pre + i, old: vec![], new: vec![] }).new.push(bo[j].to_string());
                j += 1;
            }
        }
    }
    hunks.extend(cur);
    hunks
}

/// Rebuild the text taking each hunk's new lines when `accept[i]` is true, its old lines otherwise.
pub fn apply_hunks(old: &[String], hunks: &[Hunk], accept: &[bool]) -> Vec<String> {
    let mut out = Vec::with_capacity(old.len());
    let mut pos = 0;
    for (i, h) in hunks.iter().enumerate() {
        out.extend_from_slice(&old[pos..h.old_start.min(old.len())]);
        if accept.get(i).copied().unwrap_or(false) {
            out.extend(h.new.iter().cloned());
        } else {
            out.extend(h.old.iter().cloned());
        }
        pos = h.old_start + h.old.len();
    }
    out.extend_from_slice(&old[pos.min(old.len())..]);
    out
}

/// A readable diff of `old` → `new` for an approval prompt: `@@ line N @@` headers, two lines of context,
/// `-` and `+` lines, capped at `max_lines` output lines. `old = None` means a new file. Returns the text
/// and the (added, removed) line counts.
pub fn unified(old: Option<&str>, new: &str, max_lines: usize) -> (String, usize, usize) {
    let split = |t: &str| -> Vec<String> {
        let t = t.replace("\r\n", "\n");
        if t.is_empty() { return vec![]; }
        t.strip_suffix('\n').unwrap_or(&t).split('\n').map(String::from).collect()
    };
    let new_lines = split(new);
    let mut out: Vec<String> = Vec::new();
    let (added, removed);
    match old {
        None => {
            out.push(format!("new file ({} lines)", new_lines.len()));
            out.extend(new_lines.iter().map(|l| format!("+{l}")));
            (added, removed) = (new_lines.len(), 0);
        }
        Some(o) => {
            let old_lines = split(o);
            if old_lines.len() > 5000 || new_lines.len() > 5000 {
                return (format!("large change: {} lines → {} lines", old_lines.len(), new_lines.len()), new_lines.len(), old_lines.len());
            }
            let hunks = diff_hunks(&old_lines, &new_lines);
            if hunks.is_empty() {
                return ("no change to the file's lines".to_string(), 0, 0);
            }
            (added, removed) = summary(&hunks);
            let mut last_end = 0usize; // first old line not yet printed
            for (i, h) in hunks.iter().enumerate() {
                let ctx_from = h.old_start.saturating_sub(2).max(last_end);
                out.push(format!("@@ line {} @@", ctx_from + 1));
                out.extend(old_lines[ctx_from..h.old_start].iter().map(|l| format!(" {l}")));
                out.extend(h.old.iter().map(|l| format!("-{l}")));
                out.extend(h.new.iter().map(|l| format!("+{l}")));
                let end = h.old_start + h.old.len();
                let next_start = hunks.get(i + 1).map_or(old_lines.len(), |n| n.old_start);
                let ctx_to = (end + 2).min(next_start).min(old_lines.len());
                out.extend(old_lines[end.min(ctx_to)..ctx_to].iter().map(|l| format!(" {l}")));
                last_end = ctx_to.max(end);
            }
        }
    }
    if out.len() > max_lines {
        let more = out.len() - max_lines;
        out.truncate(max_lines);
        out.push(format!("… {more} more lines"));
    }
    (out.join("\n"), added, removed)
}

/// A compact "+3 −1" style summary.
pub fn summary(hunks: &[Hunk]) -> (usize, usize) {
    (hunks.iter().map(|h| h.new.len()).sum(), hunks.iter().map(|h| h.old.len()).sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(s: &str) -> Vec<String> {
        if s.is_empty() { vec![] } else { s.split(',').map(String::from).collect() }
    }

    fn check(a: &str, b: &str) -> Vec<Hunk> {
        let (a, b) = (l(a), l(b));
        let h = diff_hunks(&a, &b);
        assert_eq!(apply_hunks(&a, &h, &vec![true; h.len()]), b, "accepting everything gives the new text");
        assert_eq!(apply_hunks(&a, &h, &vec![false; h.len()]), a, "rejecting everything gives the old text");
        h
    }

    #[test]
    fn identical_and_trivial_cases() {
        assert!(check("a,b,c", "a,b,c").is_empty());
        assert!(check("", "").is_empty());
        let h = check("", "x,y");
        assert_eq!(h, vec![Hunk { old_start: 0, old: vec![], new: l("x,y") }]);
        let h = check("x,y", "");
        assert_eq!(h, vec![Hunk { old_start: 0, old: l("x,y"), new: vec![] }]);
    }

    #[test]
    fn single_changes_are_minimal() {
        assert_eq!(check("a,b,c", "a,B,c"), vec![Hunk { old_start: 1, old: l("b"), new: l("B") }]);
        assert_eq!(check("a,b,c", "a,b,c,d"), vec![Hunk { old_start: 3, old: vec![], new: l("d") }]);
        assert_eq!(check("a,b,c", "z,a,b,c"), vec![Hunk { old_start: 0, old: vec![], new: l("z") }]);
        assert_eq!(check("a,b,c", "a,c"), vec![Hunk { old_start: 1, old: l("b"), new: vec![] }]);
    }

    #[test]
    fn separate_changes_become_separate_hunks_that_can_be_chosen() {
        let a = l("one,two,three,four,five,six");
        let b = l("ONE,two,three,four,FIVE,six,seven");
        let h = diff_hunks(&a, &b);
        assert_eq!(h.len(), 3);
        assert_eq!(apply_hunks(&a, &h, &[true, false, false]), l("ONE,two,three,four,five,six"));
        assert_eq!(apply_hunks(&a, &h, &[false, true, false]), l("one,two,three,four,FIVE,six"));
        assert_eq!(apply_hunks(&a, &h, &[false, false, true]), l("one,two,three,four,five,six,seven"));
        assert_eq!(apply_hunks(&a, &h, &[true, false, true]), l("ONE,two,three,four,five,six,seven"));
        assert_eq!(summary(&h), (3, 2));
        // a short accept list means "reject the rest"
        assert_eq!(apply_hunks(&a, &h, &[true]), l("ONE,two,three,four,five,six"));
    }

    #[test]
    fn moved_blocks_and_repeated_lines() {
        check("a,b,a,b,a", "b,a,b,a,b");
        check("x,x,x,x", "x,x");
        check("x,x", "x,x,x,x");
        check("a,b,c,d,e,f,g", "e,f,g,a,b,c,d");
        check("{,},{,}", "{,x,},{,y,}");
    }

    #[test]
    fn random_files_roundtrip_and_beat_a_naive_replace() {
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move |n: usize| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % n as u64) as usize
        };
        for round in 0..300 {
            let a: Vec<String> = (0..rnd(40)).map(|_| format!("l{}", rnd(6))).collect();
            let mut b = a.clone();
            for _ in 0..rnd(6) {
                match rnd(3) {
                    0 if !b.is_empty() => {
                        let i = rnd(b.len());
                        b.remove(i);
                    }
                    1 => {
                        let i = rnd(b.len() + 1);
                        b.insert(i, format!("n{}", rnd(4)));
                    }
                    _ if !b.is_empty() => {
                        let i = rnd(b.len());
                        b[i] = format!("c{}", rnd(4));
                    }
                    _ => {}
                }
            }
            let h = diff_hunks(&a, &b);
            assert_eq!(apply_hunks(&a, &h, &vec![true; h.len()]), b, "round {round}");
            assert_eq!(apply_hunks(&a, &h, &vec![false; h.len()]), a, "round {round}");
            // hunks are ordered, non-overlapping and non-empty
            let mut end = 0;
            for x in &h {
                assert!(x.old_start >= end && (!x.old.is_empty() || !x.new.is_empty()), "round {round}");
                end = x.old_start + x.old.len();
            }
            // never changes more lines than a handful of edits could explain
            let (added, removed) = summary(&h);
            assert!(added + removed <= 2 * 6 + 2, "round {round}: +{added} -{removed}");
        }
    }

    #[test]
    fn unified_diff_text_for_approvals() {
        let (t, a, r) = unified(Some("a\nb\nc\nd\ne\nf\ng\n"), "a\nb\nC\nd\ne\nf\ng\nh\n", 50);
        assert_eq!((a, r), (2, 1));
        assert_eq!(t, "@@ line 1 @@\n a\n b\n-c\n+C\n d\n e\n@@ line 6 @@\n f\n g\n+h");
        let (t, a, r) = unified(None, "x\ny\n", 50);
        assert_eq!((t.as_str(), a, r), ("new file (2 lines)\n+x\n+y", 2, 0));
        assert_eq!(unified(Some("same\n"), "same\n", 50).0, "no change to the file's lines");
        let (t, _, _) = unified(Some(""), "a\nb\nc\nd\ne\n", 3);
        assert_eq!(t.lines().count(), 4);
        assert!(t.ends_with("… 3 more lines"), "{t}");
        // CRLF is normalised, a pure deletion shows only minus lines
        let (t, a, r) = unified(Some("a\r\nb\r\n"), "a\n", 50);
        assert_eq!((a, r), (0, 1));
        assert!(t.contains("-b") && !t.contains("+"));
        let big: String = (0..6000).map(|i| format!("{i}\n")).collect();
        assert!(unified(Some(&big), "x\n", 50).0.starts_with("large change"));
    }

    #[test]
    fn huge_unrelated_files_fall_back_to_one_hunk_quickly() {
        let a: Vec<String> = (0..4000).map(|i| format!("a{i}")).collect();
        let b: Vec<String> = (0..4000).map(|i| format!("b{i}")).collect();
        let t = std::time::Instant::now();
        let h = diff_hunks(&a, &b);
        assert!(t.elapsed().as_secs() < 5);
        assert_eq!(h.len(), 1);
        assert_eq!(apply_hunks(&a, &h, &[true]), b);
        assert_eq!(apply_hunks(&a, &h, &[false]), a);
    }
}
