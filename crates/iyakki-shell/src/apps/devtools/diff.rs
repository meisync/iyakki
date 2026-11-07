//! Line diff of two texts (longest common subsequence). Added lines start with "+ ", removed with "- ", unchanged with two
//! spaces, so the result reads without colour.

/// The most cells of the comparison table (about 64 MB).
const MAX_CELLS: usize = 16_000_000;

pub fn diff(a: &str, b: &str) -> Result<String, String> {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    if (ma.len() + 1) * (mb.len() + 1) > MAX_CELLS {
        return Err(format!("Too many different lines to compare here ({} against {}).", ma.len(), mb.len()));
    }
    let w = mb.len() + 1;
    let mut t = vec![0u32; (ma.len() + 1) * w];
    for i in (0..ma.len()).rev() {
        for j in (0..mb.len()).rev() {
            t[i * w + j] = if ma[i] == mb[j] { t[(i + 1) * w + j + 1] + 1 } else { t[(i + 1) * w + j].max(t[i * w + j + 1]) };
        }
    }
    let mut rows: Vec<(char, &str)> = a[..pre].iter().map(|l| (' ', *l)).collect();
    let (mut i, mut j) = (0, 0);
    while i < ma.len() || j < mb.len() {
        if i < ma.len() && j < mb.len() && ma[i] == mb[j] {
            rows.push((' ', ma[i]));
            i += 1;
            j += 1;
        } else if j < mb.len() && (i == ma.len() || t[i * w + j + 1] >= t[(i + 1) * w + j]) {
            rows.push(('+', mb[j]));
            j += 1;
        } else {
            rows.push(('-', ma[i]));
            i += 1;
        }
    }
    rows.extend(a[a.len() - suf..].iter().map(|l| (' ', *l)));
    let (add, del) = (rows.iter().filter(|r| r.0 == '+').count(), rows.iter().filter(|r| r.0 == '-').count());
    if add + del == 0 {
        return Ok("The two texts are the same.".into());
    }
    let mut out = format!("{add} added (+), {del} removed (-)\n");
    // long unchanged stretches are folded to keep three lines of context
    let mut k = 0;
    while k < rows.len() {
        if rows[k].0 != ' ' {
            out.push_str(&format!("{} {}\n", rows[k].0, rows[k].1));
            k += 1;
            continue;
        }
        let end = (k..rows.len()).find(|&x| rows[x].0 != ' ').unwrap_or(rows.len());
        let (lead, trail) = (if k == 0 { 0 } else { 3 }, if end == rows.len() { 0 } else { 3 });
        if end - k > lead + trail + 1 {
            rows[k..k + lead].iter().for_each(|r| out.push_str(&format!("  {}\n", r.1)));
            out.push_str(&format!("  ... {} unchanged lines\n", end - k - lead - trail));
            rows[end - trail..end].iter().for_each(|r| out.push_str(&format!("  {}\n", r.1)));
        } else {
            rows[k..end].iter().for_each(|r| out.push_str(&format!("  {}\n", r.1)));
        }
        k = end;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_added_and_removed_lines_with_prefixes() {
        let d = diff("one\ntwo\nthree\n", "one\n2\nthree\nfour\n").unwrap();
        assert!(d.starts_with("2 added (+), 1 removed (-)"), "{d}");
        assert!(d.contains("- two\n") && d.contains("+ 2\n") && d.contains("+ four\n") && d.contains("  one\n"), "{d}");
    }

    #[test]
    fn same_text_and_long_unchanged_runs() {
        assert_eq!(diff("a\nb", "a\nb").unwrap(), "The two texts are the same.");
        let a: String = (0..50).map(|i| format!("l{i}\n")).collect();
        let b = a.replace("l25\n", "x\n");
        let d = diff(&a, &b).unwrap();
        assert!(d.contains("... 44 unchanged lines") || d.contains("unchanged lines"), "{d}");
        assert!(d.contains("- l25") && d.contains("+ x") && d.lines().count() < 20, "{d}");
        assert!(diff("", "x").unwrap().contains("+ x"));
    }

    #[test]
    fn a_huge_difference_is_refused_not_frozen() {
        let a: String = (0..5000).map(|i| format!("a{i}\n")).collect();
        let b: String = (0..5000).map(|i| format!("b{i}\n")).collect();
        assert!(diff(&a, &b).unwrap_err().contains("Too many"));
    }
}
