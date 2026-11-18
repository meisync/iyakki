//! A jq / JSONPath style query over a parsed JSON tree: `.items[2].name`, `.users[*].email`, `..id`, `[-1]`, `[1:3]`,
//! `['odd key']` and filters `[?(@.age>30)]`, `[?(@.name=="x")]`, `[?(@.admin)]`. The document is parsed once and kept for the
//! next query over the same text, so refining a path over a multi-megabyte document is fast.

use super::json::{self, Json};
use crate::safeclamp::SafeClamp;
use std::cell::RefCell;

#[derive(Debug, Clone, PartialEq)]
enum Step {
    Key(String),
    Index(i64),
    Slice(Option<i64>, Option<i64>),
    All,
    Descend(Option<String>),
    Filter(Cond),
}

#[derive(Debug, Clone, PartialEq)]
struct Cond {
    path: Vec<String>,
    test: Option<(Op, Lit)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Debug, Clone, PartialEq)]
enum Lit {
    Num(f64),
    Str(String),
    Bool(bool),
    Null,
}

/// The most matches collected (the page says when there were more).
pub const MAX_MATCHES: usize = 1_000_000;

fn parse_path(p: &str) -> Result<Vec<Step>, String> {
    let b: Vec<char> = p.trim().chars().collect();
    let mut i = 0;
    let mut steps = Vec::new();
    if b.first() == Some(&'$') {
        i = 1;
    }
    let ident = |i: &mut usize| {
        let s = *i;
        while *i < b.len() && !matches!(b[*i], '.' | '[' | ' ') {
            *i += 1;
        }
        b[s..*i].iter().collect::<String>()
    };
    while i < b.len() {
        match b[i] {
            '.' if b.get(i + 1) == Some(&'.') => {
                i += 2;
                if b.get(i) == Some(&'*') {
                    i += 1;
                    steps.push(Step::Descend(None));
                } else if b.get(i) == Some(&'[') {
                    steps.push(Step::Descend(None));
                } else {
                    let k = ident(&mut i);
                    if k.is_empty() {
                        return Err("Expected a name after '..'".into());
                    }
                    steps.push(Step::Descend(Some(k)));
                }
            }
            '.' => {
                i += 1;
                if b.get(i) == Some(&'*') {
                    i += 1;
                    steps.push(Step::All);
                } else if i >= b.len() {
                    // a trailing '.' (or just ".") means the thing itself
                } else {
                    let k = ident(&mut i);
                    if k.is_empty() {
                        return Err("Expected a name after '.'".into());
                    }
                    steps.push(Step::Key(k));
                }
            }
            '[' => {
                let close = (i..b.len()).find(|&j| b[j] == ']' && !in_quote(&b[i..j])).ok_or("Missing ']'")?;
                let inner: String = b[i + 1..close].iter().collect();
                steps.push(bracket(inner.trim())?);
                i = close + 1;
            }
            c if i == 0 && c != ' ' => {
                let k = ident(&mut i);
                steps.push(Step::Key(k));
            }
            c => return Err(format!("Unexpected '{c}' in the path")),
        }
    }
    Ok(steps)
}

/// Whether `s` (the text since an opening bracket) ends inside a quote.
fn in_quote(s: &[char]) -> bool {
    let (mut q, mut open) = (' ', false);
    for &c in s {
        if open && c == q {
            open = false;
        } else if !open && (c == '\'' || c == '"') {
            open = true;
            q = c;
        }
    }
    open
}

fn bracket(t: &str) -> Result<Step, String> {
    if t == "*" {
        return Ok(Step::All);
    }
    if let Some(f) = t.strip_prefix('?') {
        let f = f.trim();
        let f = f.strip_prefix('(').and_then(|x| x.strip_suffix(')')).unwrap_or(f);
        return Ok(Step::Filter(cond(f.trim())?));
    }
    if (t.starts_with('\'') && t.ends_with('\'') || t.starts_with('"') && t.ends_with('"')) && t.len() >= 2 {
        return Ok(Step::Key(t[1..t.len() - 1].to_string()));
    }
    if let Some((a, b)) = t.split_once(':') {
        let n = |s: &str| if s.trim().is_empty() { Ok(None) } else { s.trim().parse::<i64>().map(Some).map_err(|_| format!("Bad slice '{t}'")) };
        return Ok(Step::Slice(n(a)?, n(b)?));
    }
    t.parse::<i64>().map(Step::Index).map_err(|_| format!("Cannot read '[{t}]'"))
}

fn cond(f: &str) -> Result<Cond, String> {
    let f = f.strip_prefix('@').ok_or("A filter starts with @, like [?(@.age>30)]")?;
    let at = ["==", "!=", ">=", "<=", ">", "<"].iter().filter_map(|o| find_op(f, o).map(|p| (p, *o))).min_by_key(|(p, o)| (*p, usize::MAX - o.len()));
    let (lhs, test) = match at {
        None => (f, None),
        Some((p, o)) => {
            let op = match o {
                "==" => Op::Eq,
                "!=" => Op::Ne,
                ">=" => Op::Ge,
                "<=" => Op::Le,
                ">" => Op::Gt,
                _ => Op::Lt,
            };
            (&f[..p], Some((op, lit(f[p + o.len()..].trim())?)))
        }
    };
    let path = lhs.trim().split('.').filter(|s| !s.is_empty()).map(str::to_string).collect();
    Ok(Cond { path, test })
}

/// Position of operator `o` outside quotes.
fn find_op(f: &str, o: &str) -> Option<usize> {
    let mut q: Option<char> = None;
    for (i, c) in f.char_indices() {
        match q {
            Some(x) if c == x => q = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => q = Some(c),
            None if f[i..].starts_with(o) => return Some(i),
            None => {}
        }
    }
    None
}

fn lit(s: &str) -> Result<Lit, String> {
    match s {
        "true" => Ok(Lit::Bool(true)),
        "false" => Ok(Lit::Bool(false)),
        "null" => Ok(Lit::Null),
        _ if s.len() >= 2 && (s.starts_with('\'') && s.ends_with('\'') || s.starts_with('"') && s.ends_with('"')) => Ok(Lit::Str(s[1..s.len() - 1].to_string())),
        _ => s.parse::<f64>().map(Lit::Num).map_err(|_| format!("Cannot read the value '{s}' (put text in quotes)")),
    }
}

fn get<'a>(v: &'a Json, k: &str) -> Option<&'a Json> {
    match v {
        Json::Obj(m) => m.iter().find(|(n, _)| n == k).map(|(_, x)| x),
        _ => None,
    }
}

fn holds(v: &Json, c: &Cond) -> bool {
    let mut cur = v;
    for k in &c.path {
        match get(cur, k) {
            Some(x) => cur = x,
            // a missing field is "not equal" to anything
            None => return matches!(c.test, Some((Op::Ne, _))),
        }
    }
    let Some((op, l)) = &c.test else { return !matches!(cur, Json::Null | Json::Bool(false)) };
    use std::cmp::Ordering::*;
    let ord = match (cur, l) {
        (Json::Num(n), Lit::Num(x)) => n.parse::<f64>().ok().and_then(|n| n.partial_cmp(x)),
        (Json::Str(s), Lit::Str(x)) => Some(s.as_str().cmp(x.as_str())),
        (Json::Bool(a), Lit::Bool(b)) => Some(a.cmp(b)),
        (Json::Null, Lit::Null) => Some(Equal),
        _ => None,
    };
    match (op, ord) {
        (Op::Ne, None) => true,
        (_, None) => false,
        (Op::Eq, Some(o)) => o == Equal,
        (Op::Ne, Some(o)) => o != Equal,
        (Op::Gt, Some(o)) => o == Greater,
        (Op::Ge, Some(o)) => o != Less,
        (Op::Lt, Some(o)) => o == Less,
        (Op::Le, Some(o)) => o != Greater,
    }
}

fn children<'a>(v: &'a Json, f: &mut impl FnMut(&'a Json)) {
    match v {
        Json::Arr(a) => a.iter().for_each(f),
        Json::Obj(m) => m.iter().for_each(|(_, x)| f(x)),
        _ => {}
    }
}

fn descend<'a>(v: &'a Json, name: &Option<String>, out: &mut Vec<&'a Json>) {
    if out.len() >= MAX_MATCHES {
        return;
    }
    match name {
        None => out.push(v),
        Some(k) => {
            if let Json::Obj(m) = v {
                for (n, x) in m {
                    if n == k {
                        out.push(x);
                    }
                }
            }
        }
    }
    children(v, &mut |c| descend(c, name, out));
}

fn step<'a>(cur: Vec<&'a Json>, st: &Step) -> Vec<&'a Json> {
    let mut out: Vec<&Json> = Vec::new();
    for v in cur {
        if out.len() >= MAX_MATCHES {
            break;
        }
        match st {
            Step::Key(k) => out.extend(get(v, k)),
            Step::Index(n) => {
                if let Json::Arr(a) = v {
                    let i = if *n < 0 { a.len() as i64 + n } else { *n };
                    out.extend(usize::try_from(i).ok().and_then(|i| a.get(i)));
                }
            }
            Step::Slice(a, b) => {
                if let Json::Arr(x) = v {
                    let n = x.len() as i64;
                    let fix = |i: i64| (if i < 0 { n + i } else { i }).clamp_safe(0, n) as usize;
                    let (s, e) = (a.map_or(0, fix), b.map_or(x.len(), fix));
                    if s < e {
                        out.extend(&x[s..e]);
                    }
                }
            }
            Step::All => children(v, &mut |c| out.push(c)),
            Step::Descend(n) => descend(v, n, &mut out),
            Step::Filter(c) => children(v, &mut |x| {
                if holds(x, c) {
                    out.push(x)
                }
            }),
        }
    }
    out
}

pub fn run<'a>(root: &'a Json, path: &str) -> Result<Vec<&'a Json>, String> {
    let mut cur = vec![root];
    for s in parse_path(path)? {
        cur = step(cur, &s);
    }
    Ok(cur)
}

thread_local! {
    /// The last document parsed: (a fingerprint of its text, the tree).
    static CACHE: RefCell<Option<((usize, u64), std::rc::Rc<Json>)>> = const { RefCell::new(None) };
}

fn fingerprint(s: &str) -> (usize, u64) {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    (s.len(), h)
}

fn parsed(input: &str) -> Result<std::rc::Rc<Json>, String> {
    let fp = fingerprint(input);
    if let Some(d) = CACHE.with(|c| c.borrow().as_ref().filter(|(k, _)| *k == fp).map(|(_, d)| d.clone())) {
        return Ok(d);
    }
    let d = std::rc::Rc::new(json::parse(input).map_err(|e| format!("Invalid JSON: {e}"))?);
    CACHE.with(|c| *c.borrow_mut() = Some((fp, d.clone())));
    Ok(d)
}

/// The page action: the match count first, then up to `limit` matches (scalars on one line, containers pretty-printed).
pub fn query(input: &str, path: &str, limit: usize) -> Result<String, String> {
    let doc = parsed(input)?;
    let hits = run(&doc, path)?;
    let mut out = format!("{} match{}{}\n", hits.len(), if hits.len() == 1 { "" } else { "es" }, if hits.len() >= MAX_MATCHES { " (stopped at the limit)" } else { "" });
    for h in hits.iter().take(limit) {
        out.push_str(&json::to_string(h, Some(2)));
        out.push('\n');
    }
    if hits.len() > limit {
        out.push_str(&format!("... {} more not shown (Show more, or narrow the path)\n", hits.len() - limit));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"items":[{"id":1,"name":"a","age":20,"tags":["x"]},{"id":2,"name":"b","age":40},{"id":3,"name":"c","age":31,"admin":true}],"meta":{"id":9,"odd key":5}}"#;

    fn q(p: &str) -> Vec<String> {
        let d = json::parse(DOC).unwrap();
        run(&d, p).unwrap().iter().map(|j| json::to_string(j, None)).collect()
    }

    #[test]
    fn paths_pick_keys_indexes_slices_and_wildcards() {
        assert_eq!(q(".items[2].name"), ["\"c\""]);
        assert_eq!(q("$.items[-1].id"), ["3"]);
        assert_eq!(q(".items[*].name"), ["\"a\"", "\"b\"", "\"c\""]);
        assert_eq!(q(".items[0:2].id"), ["1", "2"]);
        assert_eq!(q(".meta['odd key']"), ["5"]);
        assert_eq!(q(".nothing.here"), Vec::<String>::new());
        assert_eq!(q(".meta.*"), ["9", "5"]);
    }

    #[test]
    fn recursive_descent_and_filters() {
        assert_eq!(q("..id"), ["1", "2", "3", "9"]);
        assert_eq!(q(".items[?(@.age>30)].name"), ["\"b\"", "\"c\""]);
        assert_eq!(q(".items[?(@.name==\"a\")].id"), ["1"]);
        assert_eq!(q(".items[?(@.admin)].id"), ["3"]);
        assert_eq!(q(".items[?(@.age<=31)].id"), ["1", "3"]);
        assert_eq!(q(".items[?(@.admin != true)].id"), ["1", "2"]);
    }

    #[test]
    fn bad_paths_say_why() {
        let d = json::parse(DOC).unwrap();
        assert!(run(&d, ".items[").unwrap_err().contains("']'"));
        assert!(run(&d, ".items[?(age>3)]").unwrap_err().contains("@"));
        assert!(query("{", ".a", 10).unwrap_err().starts_with("Invalid JSON"));
    }

    #[test]
    fn a_five_megabyte_document_is_parsed_once_and_queried_fast() {
        let mut s = String::from("{\"items\":[");
        let mut i = 0;
        while s.len() < 5_000_000 {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!("{{\"id\":{i},\"name\":\"user{i}\",\"email\":\"u{i}@example.com\",\"age\":{},\"tags\":[\"a\",\"b\"]}}", 18 + i % 60));
            i += 1;
        }
        s.push_str("]}");
        assert!(s.len() >= 5_000_000);
        let t = std::time::Instant::now();
        let out = query(&s, ".items[*].email", 50).unwrap();
        let first = t.elapsed();
        assert!(out.starts_with(&format!("{i} matches\n")), "{}", &out[..40]);
        assert!(out.contains("more not shown"));
        let t = std::time::Instant::now();
        let out = query(&s, "..id", 10).unwrap();
        assert!(out.starts_with(&format!("{i} matches")));
        let out2 = query(&s, ".items[?(@.age>70)].name", 10).unwrap();
        assert!(out2.contains("user"), "{out2}");
        assert!(query(&s, ".items[100000000].id", 10).unwrap().starts_with("0 matches"));
        let again = t.elapsed();
        assert!(first.as_secs() < 20, "parse and query: {first:?}");
        assert!(again.as_secs() < 5, "three more queries over the kept tree: {again:?}");
    }
}
