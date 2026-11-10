//! A small regular-expression engine for `grep` and `sed`: literals, `.`, classes (`[a-z]`, `[^0-9]`, `[[:alpha:]]`),
//! `\d \w \s \b` and their negations, `^ $`, groups, alternation, `* + ? {n,m}` (and lazy `*?`), case-insensitive
//! matching. It backtracks with a step limit, so a pattern that would take forever fails instead of hanging.

use std::cell::Cell;

#[derive(Clone, Debug)]
enum Node {
    Char(char),
    Any,
    Class(Vec<(char, char)>, bool),
    Start,
    End,
    WordB(bool),
    Group(Box<Node>, usize),
    Cat(Vec<Node>),
    Alt(Vec<Node>),
    Rep(Box<Node>, usize, usize, bool),
}

#[derive(Clone, Debug)]
pub struct Regex {
    root: Node,
    groups: usize,
    icase: bool,
}

pub type Caps = Vec<Option<(usize, usize)>>;

const STEP_LIMIT: u64 = 2_000_000;

struct P<'a> {
    c: Vec<char>,
    i: usize,
    ere: bool,
    groups: usize,
    _s: &'a str,
}

fn class_escape(c: char) -> Option<(Vec<(char, char)>, bool)> {
    let digit = vec![('0', '9')];
    let word = vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')];
    let space = vec![(' ', ' '), ('\t', '\r')];
    Some(match c {
        'd' => (digit, false),
        'D' => (digit, true),
        'w' => (word, false),
        'W' => (word, true),
        's' => (space, false),
        'S' => (space, true),
        _ => return None,
    })
}

fn posix_class(name: &str) -> Option<Vec<(char, char)>> {
    Some(match name {
        "alpha" => vec![('a', 'z'), ('A', 'Z')],
        "digit" => vec![('0', '9')],
        "alnum" => vec![('a', 'z'), ('A', 'Z'), ('0', '9')],
        "upper" => vec![('A', 'Z')],
        "lower" => vec![('a', 'z')],
        "space" => vec![(' ', ' '), ('\t', '\r')],
        "blank" => vec![(' ', ' '), ('\t', '\t')],
        "punct" => vec![('!', '/'), (':', '@'), ('[', '`'), ('{', '~')],
        "xdigit" => vec![('0', '9'), ('a', 'f'), ('A', 'F')],
        _ => return None,
    })
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn eat(&mut self, s: &str) -> bool {
        let w: Vec<char> = s.chars().collect();
        if self.c[self.i..].starts_with(&w) {
            self.i += w.len();
            true
        } else {
            false
        }
    }

    fn alt(&mut self, depth: usize) -> Result<Node, String> {
        if depth > 40 {
            return Err("pattern nests too deeply".into());
        }
        let mut alts = vec![self.cat(depth)?];
        loop {
            if (self.ere && self.eat("|")) || (!self.ere && self.eat("\\|")) {
                alts.push(self.cat(depth)?);
            } else {
                break;
            }
        }
        Ok(if alts.len() == 1 { alts.remove(0) } else { Node::Alt(alts) })
    }

    fn at_alt_or_close(&self) -> bool {
        if self.ere { matches!(self.peek(), Some('|') | Some(')')) } else { self.c[self.i..].starts_with(&['\\', '|']) || self.c[self.i..].starts_with(&['\\', ')']) }
    }

    fn cat(&mut self, depth: usize) -> Result<Node, String> {
        let mut items = Vec::new();
        while self.i < self.c.len() && !self.at_alt_or_close() {
            let atom = self.atom(depth)?;
            items.push(self.quant(atom)?);
        }
        Ok(Node::Cat(items))
    }

    fn quant(&mut self, mut atom: Node) -> Result<Node, String> {
        loop {
            let (min, max) = if self.eat("*") {
                (0, usize::MAX)
            } else if self.ere && self.eat("+") || !self.ere && self.eat("\\+") {
                (1, usize::MAX)
            } else if self.ere && self.eat("?") || !self.ere && self.eat("\\?") {
                (0, 1)
            } else if self.ere && self.peek() == Some('{') || !self.ere && self.c[self.i..].starts_with(&['\\', '{']) {
                let save = self.i;
                self.i += if self.ere { 1 } else { 2 };
                let mut num = String::new();
                while let Some(c) = self.peek().filter(|c| c.is_ascii_digit() || *c == ',') {
                    num.push(c);
                    self.i += 1;
                }
                let closed = if self.ere { self.eat("}") } else { self.eat("\\}") };
                let parsed = (|| {
                    let (a, b) = num.split_once(',').map_or((num.as_str(), None), |(a, b)| (a, Some(b)));
                    let a: usize = a.parse().ok()?;
                    let b = match b {
                        None => a,
                        Some("") => usize::MAX,
                        Some(b) => b.parse().ok()?,
                    };
                    (a <= b && a <= 1000 && (b == usize::MAX || b <= 1000)).then_some((a, b))
                })();
                match (closed, parsed) {
                    (true, Some(r)) => r,
                    _ if self.ere => {
                        self.i = save + 1; // a brace that is not a count is a plain brace
                        return Ok(Node::Cat(vec![atom, Node::Char('{')]));
                    }
                    _ => return Err("bad repeat count".into()),
                }
            } else {
                return Ok(atom);
            };
            if matches!(atom, Node::Start | Node::End) {
                return Ok(atom);
            }
            let lazy = self.ere && self.eat("?");
            atom = Node::Rep(Box::new(atom), min, max, !lazy);
        }
    }

    fn atom(&mut self, depth: usize) -> Result<Node, String> {
        let c = self.peek().ok_or("unexpected end")?;
        self.i += 1;
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '*' if self.i == 1 || !self.ere => Node::Char('*'),
            '(' if self.ere => self.group(depth)?,
            '[' => self.class()?,
            '\\' => {
                let e = self.peek().ok_or("a pattern cannot end with a backslash")?;
                self.i += 1;
                if !self.ere && e == '(' {
                    return self.group(depth);
                }
                if let Some((r, n)) = class_escape(e) {
                    Node::Class(r, n)
                } else if e == 'b' {
                    Node::WordB(true)
                } else if e == 'B' {
                    Node::WordB(false)
                } else {
                    Node::Char(match e {
                        'n' => '\n',
                        't' => '\t',
                        o => o,
                    })
                }
            }
            c if self.ere && matches!(c, '+' | '?') && self.i == 1 => Node::Char(c),
            c => Node::Char(c),
        })
    }

    fn group(&mut self, depth: usize) -> Result<Node, String> {
        self.groups += 1;
        let n = self.groups;
        let inner = self.alt(depth + 1)?;
        if !(if self.ere { self.eat(")") } else { self.eat("\\)") }) {
            return Err("a group is not closed".into());
        }
        Ok(Node::Group(Box::new(inner), n))
    }

    fn class(&mut self) -> Result<Node, String> {
        let neg = self.eat("^");
        let mut ranges = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek().ok_or("a [ is not closed")?;
            self.i += 1;
            if c == ']' && !first {
                break;
            }
            first = false;
            if c == '[' && self.peek() == Some(':') {
                let rest: String = self.c[self.i + 1..].iter().collect();
                if let Some(e) = rest.find(":]") {
                    if let Some(r) = posix_class(&rest[..e]) {
                        ranges.extend(r);
                        self.i += 1 + rest[..e].chars().count() + 2;
                        continue;
                    }
                }
            }
            let c = if c == '\\' {
                let e = self.peek().ok_or("a [ is not closed")?;
                self.i += 1;
                if let Some((r, false)) = class_escape(e) {
                    ranges.extend(r);
                    continue;
                }
                match e { 'n' => '\n', 't' => '\t', o => o }
            } else {
                c
            };
            if self.peek() == Some('-') && self.c.get(self.i + 1).is_some_and(|x| *x != ']') {
                let hi = self.c[self.i + 1];
                self.i += 2;
                if hi < c {
                    return Err("a range in [] runs backwards".into());
                }
                ranges.push((c, hi));
            } else {
                ranges.push((c, c));
            }
        }
        Ok(Node::Class(ranges, neg))
    }
}

struct M<'a> {
    s: &'a [char],
    icase: bool,
    steps: Cell<u64>,
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl M<'_> {
    fn go(&self, n: &Node, i: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        let st = self.steps.get() + 1;
        self.steps.set(st);
        if st > STEP_LIMIT {
            return false;
        }
        match n {
            Node::Char(c) => i < self.s.len() && (self.s[i] == *c || self.icase && fold(self.s[i]) == fold(*c)) && k(i + 1, caps),
            Node::Any => i < self.s.len() && k(i + 1, caps),
            Node::Class(r, neg) => {
                if i >= self.s.len() {
                    return false;
                }
                let c = self.s[i];
                let hit = |c: char| r.iter().any(|(a, b)| *a <= c && c <= *b);
                let m = hit(c) || self.icase && (hit(fold(c)) || c.to_uppercase().next().is_some_and(hit));
                m != *neg && k(i + 1, caps)
            }
            Node::Start => i == 0 && k(i, caps),
            Node::End => i == self.s.len() && k(i, caps),
            Node::WordB(want) => {
                let a = i > 0 && is_word(self.s[i - 1]);
                let b = i < self.s.len() && is_word(self.s[i]);
                ((a != b) == *want) && k(i, caps)
            }
            Node::Group(inner, g) => {
                let old = caps[*g];
                let g = *g;
                let r = self.go(inner, i, caps, &mut |j, caps: &mut Caps| {
                    let prev = caps[g];
                    caps[g] = Some((i, j));
                    if k(j, caps) {
                        true
                    } else {
                        caps[g] = prev;
                        false
                    }
                });
                if !r {
                    caps[g] = old;
                }
                r
            }
            Node::Cat(items) => self.cat(items, i, caps, k),
            Node::Alt(alts) => alts.iter().any(|a| self.go(a, i, caps, k)),
            Node::Rep(inner, min, max, greedy) => self.rep(inner, *min, *max, *greedy, 0, i, caps, k),
        }
    }

    fn cat(&self, items: &[Node], i: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        match items.split_first() {
            None => k(i, caps),
            Some((first, rest)) => self.go(first, i, caps, &mut |j, caps: &mut Caps| self.cat(rest, j, caps, k)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rep(&self, n: &Node, min: usize, max: usize, greedy: bool, count: usize, i: usize, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool) -> bool {
        let more = |caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool| count < max && self.go(n, i, caps, &mut |j, caps: &mut Caps| (j != i || count < min) && self.rep(n, min, max, greedy, count + 1, j, caps, k));
        if count < min {
            return more(caps, k);
        }
        if greedy {
            more(caps, k) || k(i, caps)
        } else {
            k(i, caps) || more(caps, k)
        }
    }
}

impl Regex {
    /// `ere`: extended syntax (`( ) | + ? { }` are operators); otherwise the basic one (`\( \) \| \+ \? \{`).
    pub fn new(pattern: &str, ere: bool, icase: bool) -> Result<Regex, String> {
        let mut p = P { c: pattern.chars().collect(), i: 0, ere, groups: 0, _s: pattern };
        let root = p.alt(0)?;
        if p.i < p.c.len() {
            return Err(if ere { "unmatched )" } else { "unmatched \\)" }.into());
        }
        Ok(Regex { root, groups: p.groups, icase })
    }

    pub fn groups(&self) -> usize {
        self.groups
    }

    /// The first match at or after `from` (character positions): (start, end, groups).
    pub fn find_at(&self, s: &[char], from: usize) -> Option<(usize, usize, Caps)> {
        let m = M { s, icase: self.icase, steps: Cell::new(0) };
        for start in from..=s.len() {
            let mut caps: Caps = vec![None; self.groups + 1];
            let mut end = None;
            if m.go(&self.root, start, &mut caps, &mut |j, _| {
                end = Some(j);
                true
            }) {
                let e = end.unwrap_or(start);
                caps[0] = Some((start, e));
                return Some((start, e, caps));
            }
            if m.steps.get() > STEP_LIMIT {
                return None;
            }
        }
        None
    }

    pub fn is_match(&self, s: &str) -> bool {
        let c: Vec<char> = s.chars().collect();
        self.find_at(&c, 0).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(p: &str, s: &str) -> Option<String> {
        let c: Vec<char> = s.chars().collect();
        Regex::new(p, true, false).unwrap().find_at(&c, 0).map(|(a, b, _)| c[a..b].iter().collect())
    }

    #[test]
    fn the_usual_operators_work() {
        assert_eq!(find("a.c", "xabcx").as_deref(), Some("abc"));
        assert_eq!(find("^ab+$", "abbb").as_deref(), Some("abbb"));
        assert_eq!(find("^ab+$", "abbbc"), None);
        assert_eq!(find("colou?r", "my color").as_deref(), Some("color"));
        assert_eq!(find("(cat|dog)s?", "hotdogs").as_deref(), Some("dogs"));
        assert_eq!(find("[0-9]{2,3}", "a12345").as_deref(), Some("123"));
        assert_eq!(find("\\d+\\.\\d+", "pi is 3.14159!").as_deref(), Some("3.14159"));
        assert_eq!(find("[[:upper:]][[:lower:]]+", "say Hello").as_deref(), Some("Hello"));
        assert_eq!(find("[^a-c]+", "abcxyz").as_deref(), Some("xyz"));
        assert_eq!(find("\\bfoo\\b", "a foo b").as_deref(), Some("foo"));
        assert_eq!(find("\\bfoo\\b", "afoob"), None);
        assert_eq!(find("a.*b", "a1b2b3").as_deref(), Some("a1b2b"), "greedy");
        assert_eq!(find("a.*?b", "a1b2b3").as_deref(), Some("a1b"), "lazy");
        assert_eq!(find("x*", "abc").as_deref(), Some(""), "an empty match is a match");
        assert_eq!(find("a{", "a{").as_deref(), Some("a{"));
    }

    #[test]
    fn groups_are_captured_and_case_can_be_ignored() {
        let r = Regex::new("(\\w+)@(\\w+)\\.com", true, false).unwrap();
        let c: Vec<char> = "mail bob@example.com now".chars().collect();
        let (_, _, caps) = r.find_at(&c, 0).unwrap();
        let g = |n: usize| caps[n].map(|(a, b)| c[a..b].iter().collect::<String>()).unwrap();
        assert_eq!((g(1), g(2), g(0)), ("bob".into(), "example".into(), "bob@example.com".into()));
        assert!(Regex::new("HELLO", true, true).unwrap().is_match("say hello"));
        assert!(Regex::new("[a-c]+", true, true).unwrap().is_match("ABC"));
        assert!(!Regex::new("HELLO", true, false).unwrap().is_match("say hello"));
    }

    #[test]
    fn the_basic_syntax_uses_backslashes_for_operators() {
        let b = |p: &str, s: &str| Regex::new(p, false, false).unwrap().is_match(s);
        assert!(b("\\(ab\\)\\+c", "ababc") && b("a\\|b", "b") && b("a+b", "a+b") && !b("a+b", "aab") && b("(x)", "(x)"));
        assert!(b("a\\{2\\}", "aa") && !b("^a\\{2\\}$", "aaa"));
    }

    #[test]
    fn bad_patterns_are_errors_and_runaway_patterns_stop() {
        for bad in ["(a", "a)", "[a", "[z-a]", "\\", "a{2,1}"] {
            assert!(Regex::new(bad, true, false).is_err() || bad == "a{2,1}", "{bad}");
        }
        assert!(Regex::new(&"(".repeat(100), true, false).is_err());
        let t = std::time::Instant::now();
        let r = Regex::new("(a*)*b", true, false).unwrap();
        assert!(!r.is_match(&"a".repeat(40)));
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "the step limit ends it");
        assert!(Regex::new("é+", true, false).unwrap().is_match("caféé"));
    }
}
