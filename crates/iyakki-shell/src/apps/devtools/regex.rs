//! A small backtracking regular-expression matcher for the tester: `. * + ? *? +? ?? {m} {m,} {m,n}`, `[...]`, `[^...]`,
//! ranges, `\d \w \s \D \W \S \b`, `^ $`, `|`, `(...)` and `(?:...)` groups, and an ignore-case flag. The work is counted:
//! past [`STEP_LIMIT`] steps the search stops and says so (a pattern like `(a+)+b` can take forever otherwise).

use std::cell::Cell;

/// Steps a whole search may take.
pub const STEP_LIMIT: usize = 2_000_000;
/// The deepest the matcher nests (a stack guard).
const DEPTH_LIMIT: usize = 3000;
/// The longest input searched (characters).
pub const MAX_INPUT: usize = 200_000;

#[derive(Debug, Clone)]
enum Node {
    Char(char),
    Any,
    Class(Vec<(char, char)>, bool),
    Start,
    End,
    WordB,
    /// Capture index (None: no capture) and the alternatives.
    Group(Option<usize>, Vec<Vec<Node>>),
    Repeat(Box<Node>, usize, usize, bool),
}

pub struct Regex {
    alts: Vec<Vec<Node>>,
    groups: usize,
    icase: bool,
}

struct Parser {
    c: Vec<char>,
    i: usize,
    groups: usize,
}

fn class_of(c: char) -> Option<(Vec<(char, char)>, bool)> {
    Some(match c {
        'd' => (vec![('0', '9')], false),
        'D' => (vec![('0', '9')], true),
        'w' => (vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')], false),
        'W' => (vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')], true),
        's' => (vec![(' ', ' '), ('\t', '\r')], false),
        'S' => (vec![(' ', ' '), ('\t', '\r')], true),
        _ => return None,
    })
}

fn escaped(c: char) -> char {
    match c {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        c => c,
    }
}

impl Parser {
    fn alts(&mut self, depth: usize) -> Result<Vec<Vec<Node>>, String> {
        if depth > 100 {
            return Err("Groups nested too deeply".into());
        }
        let mut alts = vec![Vec::new()];
        while let Some(&c) = self.c.get(self.i) {
            match c {
                ')' if depth > 0 => break,
                ')' => return Err("Unmatched ')'".into()),
                '|' => {
                    self.i += 1;
                    alts.push(Vec::new());
                    continue;
                }
                _ => {}
            }
            let atom = self.atom(depth)?;
            let atom = self.quant(atom)?;
            alts.last_mut().unwrap().push(atom);
        }
        Ok(alts)
    }

    fn atom(&mut self, depth: usize) -> Result<Node, String> {
        let c = self.c[self.i];
        self.i += 1;
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                let cap = if self.c[self.i..].starts_with(&['?', ':']) {
                    self.i += 2;
                    None
                } else {
                    self.groups += 1;
                    Some(self.groups)
                };
                let a = self.alts(depth + 1)?;
                if self.c.get(self.i) != Some(&')') {
                    return Err("Missing ')'".into());
                }
                self.i += 1;
                Node::Group(cap, a)
            }
            '[' => self.class()?,
            '\\' => {
                let e = *self.c.get(self.i).ok_or("A backslash at the end")?;
                self.i += 1;
                match class_of(e) {
                    Some((r, n)) => Node::Class(r, n),
                    None if e == 'b' => Node::WordB,
                    None => Node::Char(escaped(e)),
                }
            }
            '*' | '+' | '?' => return Err(format!("Nothing to repeat before '{c}'")),
            c => Node::Char(c),
        })
    }

    fn class(&mut self) -> Result<Node, String> {
        let neg = self.c.get(self.i) == Some(&'^');
        if neg {
            self.i += 1;
        }
        let mut r = Vec::new();
        let mut first = true;
        loop {
            let c = *self.c.get(self.i).ok_or("Missing ']'")?;
            self.i += 1;
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = if c == '\\' {
                let e = *self.c.get(self.i).ok_or("Missing ']'")?;
                self.i += 1;
                if let Some((cr, false)) = class_of(e) {
                    r.extend(cr);
                    continue;
                }
                escaped(e)
            } else {
                c
            };
            if self.c.get(self.i) == Some(&'-') && self.c.get(self.i + 1).is_some_and(|x| *x != ']') {
                let hi = self.c[self.i + 1];
                self.i += 2;
                if hi < lo {
                    return Err(format!("The range {lo}-{hi} is backwards"));
                }
                r.push((lo, hi));
            } else {
                r.push((lo, lo));
            }
        }
        Ok(Node::Class(r, neg))
    }

    fn quant(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.c.get(self.i) {
            Some('*') => (0, usize::MAX),
            Some('+') => (1, usize::MAX),
            Some('?') => (0, 1),
            Some('{') => {
                let Some(close) = self.c[self.i..].iter().position(|c| *c == '}') else { return Ok(atom) };
                let body: String = self.c[self.i + 1..self.i + close].iter().collect();
                let parsed = match body.split_once(',') {
                    None => body.parse().ok().map(|n| (n, n)),
                    Some((a, "")) => a.parse().ok().map(|n| (n, usize::MAX)),
                    Some((a, b)) => a.parse().ok().zip(b.parse().ok()),
                };
                let Some((a, b)) = parsed else { return Ok(atom) };
                if b < a {
                    return Err("In {m,n}, n is smaller than m".into());
                }
                self.i += close;
                (a, b)
            }
            _ => return Ok(atom),
        };
        self.i += 1;
        if matches!(atom, Node::Start | Node::End | Node::WordB) {
            return Err("Cannot repeat ^, $ or \\b".into());
        }
        let lazy = self.c.get(self.i) == Some(&'?');
        if lazy {
            self.i += 1;
        }
        Ok(Node::Repeat(Box::new(atom), min, max, lazy))
    }
}

/// A match: the character range and the capture groups (None: did not take part).
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
    pub groups: Vec<Option<(usize, usize)>>,
}

struct Run<'a> {
    t: &'a [char],
    icase: bool,
    steps: Cell<usize>,
    aborted: Cell<bool>,
}

type Caps = Vec<Option<(usize, usize)>>;

impl Regex {
    pub fn new(pattern: &str, icase: bool) -> Result<Regex, String> {
        let mut p = Parser { c: pattern.chars().collect(), i: 0, groups: 0 };
        let alts = p.alts(0)?;
        Ok(Regex { alts, groups: p.groups, icase })
    }

    /// All non-overlapping matches, and whether the step limit stopped the search.
    pub fn find_all(&self, text: &[char]) -> (Vec<Match>, bool) {
        let run = Run { t: text, icase: self.icase, steps: Cell::new(0), aborted: Cell::new(false) };
        let (mut out, mut at) = (Vec::new(), 0);
        while at <= text.len() && !run.aborted.get() {
            let mut caps: Caps = vec![None; self.groups + 1];
            let mut end = None;
            if run.alts(&self.alts, at, &mut caps, 0, &mut |p, _| {
                end = Some(p);
                true
            }) {
                let e = end.unwrap_or(at);
                out.push(Match { start: at, end: e, groups: caps[1..].to_vec() });
                at = if e == at { at + 1 } else { e };
            } else {
                at += 1;
            }
        }
        (out, run.aborted.get())
    }
}

impl Run<'_> {
    fn tick(&self, depth: usize) -> bool {
        let n = self.steps.get() + 1;
        self.steps.set(n);
        if n > STEP_LIMIT || depth > DEPTH_LIMIT {
            self.aborted.set(true);
        }
        self.aborted.get()
    }

    fn alts(&self, alts: &[Vec<Node>], pos: usize, caps: &mut Caps, depth: usize, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        alts.iter().any(|seq| self.seq(seq, pos, caps, depth, k))
    }

    fn seq(&self, seq: &[Node], pos: usize, caps: &mut Caps, depth: usize, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        let Some((first, rest)) = seq.split_first() else { return k(pos, caps) };
        if self.tick(depth) {
            return false;
        }
        match first {
            Node::Group(cap, alts) => {
                let cap = *cap;
                self.alts(alts, pos, caps, depth + 1, &mut |e, caps: &mut Caps| {
                    let old = cap.map(|c| caps[c]);
                    if let Some(c) = cap {
                        caps[c] = Some((pos, e));
                    }
                    if self.seq(rest, e, caps, depth + 1, k) {
                        return true;
                    }
                    if let (Some(c), Some(o)) = (cap, old) {
                        caps[c] = o;
                    }
                    false
                })
            }
            Node::Repeat(inner, min, max, lazy) => {
                if let Node::Char(_) | Node::Any | Node::Class(..) = **inner {
                    return self.repeat_simple(inner, *min, *max, *lazy, rest, pos, caps, depth, k);
                }
                self.repeat(inner, *min, *max, *lazy, 0, rest, pos, caps, depth, k)
            }
            n => match self.one(n, pos) {
                Some(p) => self.seq(rest, p, caps, depth + 1, k),
                None => false,
            },
        }
    }

    /// A single-character node or an assertion at `pos`: the position after it.
    fn one(&self, n: &Node, pos: usize) -> Option<usize> {
        let c = self.t.get(pos).copied();
        let word = |i: Option<usize>| i.and_then(|i| self.t.get(i)).is_some_and(|c| c.is_alphanumeric() || *c == '_');
        match n {
            Node::Char(x) => c.filter(|c| self.eq(*c, *x)).map(|_| pos + 1),
            Node::Any => c.filter(|c| *c != '\n').map(|_| pos + 1),
            Node::Class(r, neg) => c.filter(|c| self.in_class(r, *c) != *neg).map(|_| pos + 1),
            Node::Start => (pos == 0 || self.t[pos - 1] == '\n').then_some(pos),
            Node::End => (pos == self.t.len() || self.t[pos] == '\n').then_some(pos),
            Node::WordB => (word(pos.checked_sub(1)) != word(Some(pos))).then_some(pos),
            _ => None,
        }
    }

    fn eq(&self, a: char, b: char) -> bool {
        a == b || (self.icase && a.to_lowercase().eq(b.to_lowercase()))
    }

    fn in_class(&self, r: &[(char, char)], c: char) -> bool {
        let hit = |c: char| r.iter().any(|(lo, hi)| *lo <= c && c <= *hi);
        hit(c) || (self.icase && (c.to_lowercase().any(hit) || c.to_uppercase().any(hit)))
    }

    /// `x*` on one character: counted in a loop, so a long input does not nest deeply.
    #[allow(clippy::too_many_arguments)]
    fn repeat_simple(&self, n: &Node, min: usize, max: usize, lazy: bool, rest: &[Node], pos: usize, caps: &mut Caps, depth: usize, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        let mut ends = vec![pos];
        let mut p = pos;
        while ends.len() - 1 < max {
            match self.one(n, p) {
                Some(q) => {
                    p = q;
                    ends.push(p);
                }
                None => break,
            }
            if self.tick(depth) {
                return false;
            }
        }
        if ends.len() - 1 < min {
            return false;
        }
        let order: Vec<usize> = if lazy { (min..ends.len()).collect() } else { (min..ends.len()).rev().collect() };
        for i in order {
            if self.seq(rest, ends[i], caps, depth + 1, k) {
                return true;
            }
            if self.aborted.get() {
                return false;
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn repeat(&self, n: &Node, min: usize, max: usize, lazy: bool, count: usize, rest: &[Node], pos: usize, caps: &mut Caps, depth: usize, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        if self.tick(depth) {
            return false;
        }
        let more = |caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool| {
            count < max
                && self.seq(std::slice::from_ref(n), pos, caps, depth + 1, &mut |e, caps: &mut Caps| {
                    // an empty round cannot make progress
                    if e == pos && count >= min {
                        return false;
                    }
                    self.repeat(n, min, max, lazy, count + 1, rest, e, caps, depth + 1, k)
                })
        };
        if count < min {
            return more(caps, k);
        }
        if lazy {
            self.seq(rest, pos, caps, depth + 1, k) || more(caps, k)
        } else {
            more(caps, k) || self.seq(rest, pos, caps, depth + 1, k)
        }
    }
}

/// The page action: every match with its position and groups.
pub fn test(pattern: &str, text: &str, icase: bool) -> Result<String, String> {
    if pattern.is_empty() {
        return Err("Type a pattern in the Pattern box.".into());
    }
    let re = Regex::new(pattern, icase).map_err(|e| format!("Pattern error: {e}"))?;
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > MAX_INPUT {
        return Err(format!("The text is longer than {MAX_INPUT} characters; test on a shorter piece."));
    }
    let (ms, stopped) = re.find_all(&chars);
    let mut out = format!("{} match{}{}\n", ms.len(), if ms.len() == 1 { "" } else { "es" }, if icase { " (ignoring case)" } else { "" });
    if stopped {
        out.push_str(&format!("STOPPED: the search passed {STEP_LIMIT} steps (a pattern that backtracks too much); the matches below are only those found before that.\n"));
    }
    let s = |a: usize, b: usize| -> String { chars[a..b].iter().collect::<String>().replace('\n', "\\n") };
    for (i, m) in ms.iter().enumerate().take(500) {
        out.push_str(&format!("{}. [{}..{}] \"{}\"\n", i + 1, m.start, m.end, s(m.start, m.end)));
        for (g, r) in m.groups.iter().enumerate() {
            match r {
                Some((a, b)) => out.push_str(&format!("     group {}: \"{}\"\n", g + 1, s(*a, *b))),
                None => out.push_str(&format!("     group {}: (did not match)\n", g + 1)),
            }
        }
    }
    if ms.len() > 500 {
        out.push_str(&format!("... {} more matches not listed\n", ms.len() - 500));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(p: &str, t: &str, ic: bool) -> Vec<String> {
        let re = Regex::new(p, ic).unwrap();
        let c: Vec<char> = t.chars().collect();
        re.find_all(&c).0.iter().map(|m| c[m.start..m.end].iter().collect()).collect()
    }

    #[test]
    fn literals_classes_and_repeats() {
        assert_eq!(all("a+b", "aab ab b", false), ["aab", "ab"]);
        assert_eq!(all(r"\d{2,3}", "1 22 4444", false), ["22", "444"]);
        assert_eq!(all("[a-c]+", "abcd-cab", false), ["abc", "cab"]);
        assert_eq!(all("[^a-c ]+", "abd ce", false), ["d", "e"]);
        assert_eq!(all(r"\w+@\w+\.com", "mail bob@ex.com now", false), ["bob@ex.com"]);
        assert_eq!(all("colou?r", "color colour", false), ["color", "colour"]);
        assert_eq!(all("<.+?>", "<a><b>", false), ["<a>", "<b>"]);
        assert_eq!(all("<.+>", "<a><b>", false), ["<a><b>"]);
    }

    #[test]
    fn anchors_alternation_groups_and_case() {
        assert_eq!(all("^ab", "ab\nab ab", false), ["ab", "ab"]);
        assert_eq!(all("b$", "ab\nb b", false), ["b", "b"]);
        assert_eq!(all("cat|dog", "hotdog cat", false), ["dog", "cat"]);
        assert_eq!(all("(ab)+", "ababab x ab", false), ["ababab", "ab"]);
        assert_eq!(all(r"\bis\b", "this is it", false), ["is"]);
        assert_eq!(all("HELLO", "Hello hello", true), ["Hello", "hello"]);
        assert_eq!(all("[a-z]+", "ABC", true), ["ABC"]);
        let re = Regex::new(r"(\d+)-(x)?", false).unwrap();
        let c: Vec<char> = "12-".chars().collect();
        let (m, _) = re.find_all(&c);
        assert_eq!(m[0].groups, vec![Some((0, 2)), None]);
    }

    #[test]
    fn bad_patterns_and_catastrophic_ones_are_reported() {
        for p in ["(a", "a)", "[a-", "*a", "[z-a]", "a{3,1}"] {
            assert!(Regex::new(p, false).is_err(), "{p}");
        }
        let t = test("(a+)+$", &format!("{}b", "a".repeat(40)), false).unwrap();
        assert!(t.contains("STOPPED"), "{t}");
        let ok = test("(a|b)*c", "ababc", false).unwrap();
        assert!(ok.starts_with("1 match") && ok.contains("group 1: \"b\""), "{ok}");
        assert!(test("", "x", false).is_err());
        assert!(test("a", &"a".repeat(MAX_INPUT + 1), false).is_err());
    }

    #[test]
    fn long_input_does_not_overflow_the_stack() {
        let t = "x".repeat(100_000);
        assert_eq!(all(".*", &t, false).len(), 2, "the whole text, then the empty match at the end");
        assert!(test("(x)*y", &"x".repeat(5000), false).is_ok());
    }
}
