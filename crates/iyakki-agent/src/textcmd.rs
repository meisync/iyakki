//! The everyday text tools of a Linux terminal, written here so they work the same on every machine, take no shell
//! and can run for a person's account without any program being started: `grep sed tr cut sort uniq wc head tail tac
//! rev nl seq date basename dirname`. They read the previous command's output (a pipe) or files the account may read.

use crate::clock;
use crate::regex::Regex;

pub const COMMANDS: [&str; 16] = ["grep", "sed", "tr", "cut", "sort", "uniq", "wc", "head", "tail", "tac", "rev", "nl", "seq", "date", "basename", "dirname"];
const MAX_OUT: usize = 256 * 1024;

pub fn is_text_command(c: &str) -> bool {
    COMMANDS.contains(&c)
}

/// Reads a file the caller may read. `-` or no name means the pipe.
pub type Reader<'a> = &'a dyn Fn(&str) -> Result<String, String>;

fn lines_of(s: &str) -> Vec<&str> {
    let mut v: Vec<&str> = s.split('\n').collect();
    if v.last() == Some(&"") {
        v.pop();
    }
    v
}

fn join(lines: &[String]) -> String {
    let mut s = lines.join("\n");
    if !lines.is_empty() {
        s.push('\n');
    }
    s
}

struct Args<'a> {
    flags: Vec<char>,
    /// `-n 5` style options by letter.
    opts: Vec<(char, String)>,
    rest: Vec<&'a str>,
}

/// Splits `-abc`, `-n 5`, `-n5` and `--long` style arguments. `with_value` lists the option letters that take a value.
fn parse<'a>(args: &'a [String], with_value: &str) -> Result<Args<'a>, String> {
    let mut a = Args { flags: vec![], opts: vec![], rest: vec![] };
    let mut i = 0;
    let mut only_files = false;
    while i < args.len() {
        let s = args[i].as_str();
        if only_files || s == "-" || !s.starts_with('-') || s.len() < 2 {
            a.rest.push(s);
        } else if s == "--" {
            only_files = true;
        } else if let Some(long) = s.strip_prefix("--") {
            return Err(format!("--{long} is not supported"));
        } else if s[1..].chars().all(|c| c.is_ascii_digit()) {
            a.opts.push(('n', s[1..].to_string())); // head -5
        } else {
            let cs: Vec<char> = s[1..].chars().collect();
            let mut j = 0;
            while j < cs.len() {
                let c = cs[j];
                if with_value.contains(c) {
                    let attached: String = cs[j + 1..].iter().collect();
                    let v = if !attached.is_empty() {
                        attached
                    } else {
                        i += 1;
                        args.get(i).ok_or(format!("-{c} needs a value"))?.clone()
                    };
                    a.opts.push((c, v));
                    break;
                }
                a.flags.push(c);
                j += 1;
            }
        }
        i += 1;
    }
    Ok(a)
}

impl Args<'_> {
    fn has(&self, c: char) -> bool {
        self.flags.contains(&c)
    }
    fn opt(&self, c: char) -> Option<&str> {
        self.opts.iter().rev().find(|(k, _)| *k == c).map(|(_, v)| v.as_str())
    }
    fn num(&self, c: char, default: usize) -> Result<usize, String> {
        self.opt(c).map_or(Ok(default), |v| v.parse().map_err(|_| format!("`{v}` is not a number")))
    }
    fn unknown(&self, allowed: &str) -> Result<(), String> {
        match self.flags.iter().find(|c| !allowed.contains(**c)) {
            Some(c) => Err(format!("-{c} is not supported")),
            None => Ok(()),
        }
    }
}

/// The input: the files named, or the pipe.
fn inputs(files: &[&str], stdin: Option<&str>, read: Reader) -> Result<Vec<(String, String)>, String> {
    if files.is_empty() {
        return Ok(vec![("(standard input)".into(), stdin.ok_or("needs a file or a pipe: `cat file | …`")?.to_string())]);
    }
    files.iter().map(|f| if *f == "-" { stdin.map(|s| (f.to_string(), s.to_string())).ok_or("no pipe to read".to_string()) } else { read(f).map(|t| (f.to_string(), t)) }).collect()
}

fn one_input(files: &[&str], stdin: Option<&str>, read: Reader) -> Result<String, String> {
    Ok(inputs(files, stdin, read)?.into_iter().map(|(_, t)| t).collect::<String>())
}

/// Runs a text command. `None` means `cmd` is not one of them.
pub fn run(cmd: &str, args: &[String], stdin: Option<&str>, read: Reader) -> Option<Result<String, String>> {
    let r = match cmd {
        "grep" => grep(args, stdin, read),
        "sed" => sed(args, stdin, read),
        "tr" => tr(args, stdin),
        "cut" => cut(args, stdin, read),
        "sort" => sort(args, stdin, read),
        "uniq" => uniq(args, stdin, read),
        "wc" => wc(args, stdin, read),
        "head" | "tail" => head_tail(cmd == "head", args, stdin, read),
        "tac" => one_input(&parse(args, "").ok()?.rest, stdin, read).map(|t| join(&lines_of(&t).iter().rev().map(|s| s.to_string()).collect::<Vec<_>>())),
        "rev" => one_input(&parse(args, "").ok()?.rest, stdin, read).map(|t| join(&lines_of(&t).iter().map(|l| l.chars().rev().collect()).collect::<Vec<_>>())),
        "nl" => one_input(&parse(args, "").ok()?.rest, stdin, read).map(|t| join(&lines_of(&t).iter().enumerate().map(|(i, l)| format!("{:>6}\t{l}", i + 1)).collect::<Vec<_>>())),
        "seq" => seq(args),
        "date" => date(args),
        "basename" | "dirname" => path_part(cmd == "basename", args),
        _ => return None,
    };
    Some(r.map(|mut s| {
        if s.len() > MAX_OUT {
            let mut cut = MAX_OUT;
            while !s.is_char_boundary(cut) {
                cut -= 1;
            }
            s.truncate(cut);
            s.push_str("\n… (output cut at 256 KiB)\n");
        }
        s
    }))
}

fn compile(pat: &str, ere: bool, icase: bool) -> Result<Regex, String> {
    Regex::new(pat, ere, icase).map_err(|e| format!("bad pattern `{pat}`: {e}"))
}

fn grep(args: &[String], stdin: Option<&str>, read: Reader) -> Result<String, String> {
    let a = parse(args, "me")?;
    a.unknown("ivcnEFowxqHh")?;
    let mut rest = a.rest.clone();
    let es: Vec<&str> = a.opts.iter().filter(|(k, _)| *k == 'e').map(|(_, v)| v.as_str()).collect();
    let multi = es.len() > 1;
    let pat = if multi { es.iter().map(|e| format!("({e})")).collect::<Vec<_>>().join("|") } else if let Some(e) = es.first() { e.to_string() } else if rest.is_empty() { return Err("usage: grep [-ivcnowxEF] [-m N] pattern [file…]".into()) } else { rest.remove(0).to_string() };
    let (icase, invert, count, num, only, whole_word, whole_line) = (a.has('i'), a.has('v'), a.has('c'), a.has('n'), a.has('o'), a.has('w'), a.has('x'));
    let pattern = if a.has('F') {
        pat.chars().flat_map(|c| if "\\.[]()*+?{}|^$".contains(c) { vec!['\\', c] } else { vec![c] }).collect::<String>()
    } else {
        pat
    };
    let pattern = match (whole_word, whole_line) {
        (_, true) => format!("^({pattern})$"),
        (true, _) => format!("\\b({pattern})\\b"),
        _ => pattern,
    };
    let re = compile(&pattern, multi || a.has('E') || a.has('F') || whole_word || whole_line, icase)?;
    let limit = a.num('m', usize::MAX)?;
    let ins = inputs(&rest, stdin, read)?;
    let many = ins.len() > 1 && !a.has('h');
    let mut out = Vec::new();
    let mut any = false;
    for (name, text) in &ins {
        let mut n = 0;
        for (i, line) in lines_of(text).iter().enumerate() {
            if n >= limit {
                break;
            }
            let chars: Vec<char> = line.chars().collect();
            let hit = re.find_at(&chars, 0).is_some();
            if hit == invert {
                continue;
            }
            n += 1;
            any = true;
            let prefix = format!("{}{}", if many { format!("{name}:") } else { String::new() }, if num { format!("{}:", i + 1) } else { String::new() });
            if only && !invert {
                let mut at = 0;
                while let Some((s, e, _)) = re.find_at(&chars, at) {
                    if e > s {
                        out.push(format!("{prefix}{}", chars[s..e].iter().collect::<String>()));
                    }
                    at = e.max(s + 1);
                    if at > chars.len() {
                        break;
                    }
                }
            } else if !count {
                out.push(format!("{prefix}{line}"));
            }
        }
        if count {
            out.push(format!("{}{n}", if many { format!("{name}:") } else { String::new() }));
        }
    }
    if a.has('q') {
        return Ok(String::new());
    }
    if !any && !count {
        return Ok(String::new());
    }
    Ok(join(&out))
}

// ------------------------------------------------------------------------------------------------------------- sed

enum Addr {
    Line(usize),
    Last,
    Re(Regex),
}

enum Act {
    Sub { re: Regex, repl: Vec<ReplPart>, global: bool, nth: usize, print: bool },
    Delete,
    Print,
    Translate(Vec<char>, Vec<char>),
    Quit,
}

enum ReplPart {
    Lit(String),
    Group(usize),
}

struct Cmd {
    a1: Option<Addr>,
    a2: Option<Addr>,
    negate: bool,
    act: Act,
}

fn parse_replacement(r: &str, groups: usize) -> Result<Vec<ReplPart>, String> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut it = r.chars();
    while let Some(c) = it.next() {
        match c {
            '&' => {
                if !lit.is_empty() {
                    out.push(ReplPart::Lit(std::mem::take(&mut lit)));
                }
                out.push(ReplPart::Group(0));
            }
            '\\' => match it.next() {
                Some(d) if d.is_ascii_digit() => {
                    let n = d as usize - '0' as usize;
                    if n > groups {
                        return Err(format!("the replacement uses \\{n} but the pattern has {groups} group(s)"));
                    }
                    if !lit.is_empty() {
                        out.push(ReplPart::Lit(std::mem::take(&mut lit)));
                    }
                    out.push(ReplPart::Group(n));
                }
                Some('n') => lit.push('\n'),
                Some('t') => lit.push('\t'),
                Some(o) => lit.push(o),
                None => lit.push('\\'),
            },
            c => lit.push(c),
        }
    }
    if !lit.is_empty() {
        out.push(ReplPart::Lit(lit));
    }
    Ok(out)
}

/// Reads up to an unescaped `delim`; returns the text and what follows.
fn until(s: &[char], delim: char) -> Result<(String, &[char]), String> {
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        if s[i] == '\\' && i + 1 < s.len() {
            if s[i + 1] == delim && !"|()+?{}.*[]^$".contains(delim) {
                out.push(delim);
            } else {
                out.push('\\');
                out.push(s[i + 1]);
            }
            i += 2;
            continue;
        }
        if s[i] == delim {
            return Ok((out, &s[i + 1..]));
        }
        out.push(s[i]);
        i += 1;
    }
    Err(format!("the command is missing a closing `{delim}`"))
}

fn parse_script(script: &str, ere: bool) -> Result<Vec<Cmd>, String> {
    let c: Vec<char> = script.chars().collect();
    let mut s: &[char] = &c;
    let mut cmds = Vec::new();
    loop {
        while let Some(f) = s.first() {
            if f.is_whitespace() || *f == ';' || *f == '\n' {
                s = &s[1..];
            } else {
                break;
            }
        }
        if s.is_empty() {
            break;
        }
        let addr = |s: &mut &[char]| -> Result<Option<Addr>, String> {
            match s.first() {
                Some(d) if d.is_ascii_digit() => {
                    let n: String = s.iter().take_while(|c| c.is_ascii_digit()).collect();
                    *s = &s[n.len()..];
                    Ok(Some(Addr::Line(n.parse().map_err(|_| "bad line number".to_string())?)))
                }
                Some('$') => {
                    *s = &s[1..];
                    Ok(Some(Addr::Last))
                }
                Some('/') => {
                    let (p, rest) = until(&s[1..], '/')?;
                    let icase = rest.first() == Some(&'I');
                    *s = if icase { &rest[1..] } else { rest };
                    Ok(Some(Addr::Re(compile(&p, ere, icase)?)))
                }
                _ => Ok(None),
            }
        };
        let a1 = addr(&mut s)?;
        let a2 = if a1.is_some() && s.first() == Some(&',') {
            s = &s[1..];
            Some(addr(&mut s)?.ok_or("a range needs an end")?)
        } else {
            None
        };
        let mut negate = false;
        while s.first() == Some(&'!') {
            negate = true;
            s = &s[1..];
        }
        while s.first() == Some(&' ') {
            s = &s[1..];
        }
        let op = *s.first().ok_or("an address needs a command")?;
        s = &s[1..];
        let act = match op {
            'd' => Act::Delete,
            'p' => Act::Print,
            'q' => Act::Quit,
            's' => {
                let delim = *s.first().ok_or("s needs a pattern")?;
                if delim == '\\' || delim == '\n' {
                    return Err("bad delimiter after s".into());
                }
                let (pat, rest) = until(&s[1..], delim)?;
                let (repl, rest) = until(rest, delim)?;
                let flags: String = rest.iter().take_while(|c| c.is_alphanumeric()).collect();
                s = &rest[flags.chars().count()..];
                let (mut global, mut icase, mut print, mut nth) = (false, false, false, 0);
                let mut digits = String::new();
                for f in flags.chars() {
                    match f {
                        'g' => global = true,
                        'i' | 'I' => icase = true,
                        'p' => print = true,
                        d if d.is_ascii_digit() => digits.push(d),
                        o => return Err(format!("unknown flag `{o}` on s")),
                    }
                }
                if !digits.is_empty() {
                    nth = digits.parse().map_err(|_| "bad number flag")?;
                }
                let re = compile(&pat, ere, icase)?;
                let repl = parse_replacement(&repl, re.groups())?;
                Act::Sub { re, repl, global, nth, print }
            }
            'y' => {
                let delim = *s.first().ok_or("y needs two sets")?;
                let (a, rest) = until(&s[1..], delim)?;
                let (b, rest) = until(rest, delim)?;
                s = rest;
                let (a, b): (Vec<char>, Vec<char>) = (expand_set(&a)?, expand_set(&b)?);
                if a.len() != b.len() {
                    return Err("y needs two sets of the same length".into());
                }
                Act::Translate(a, b)
            }
            o => return Err(format!("unknown command `{o}` (supported: s d p q y)")),
        };
        cmds.push(Cmd { a1, a2, negate, act });
    }
    Ok(cmds)
}

fn addr_hit(a: &Addr, n: usize, last: bool, line: &str) -> bool {
    match a {
        Addr::Line(l) => *l == n,
        Addr::Last => last,
        Addr::Re(r) => r.is_match(line),
    }
}

fn sed(args: &[String], stdin: Option<&str>, read: Reader) -> Result<String, String> {
    let a = parse(args, "e")?;
    a.unknown("nEr")?;
    let mut rest = a.rest.clone();
    let scripts: Vec<&str> = a.opts.iter().filter(|(k, _)| *k == 'e').map(|(_, v)| v.as_str()).collect();
    let script = if scripts.is_empty() {
        if rest.is_empty() {
            return Err("usage: sed [-n] [-E] 's/old/new/g' [file…]   (also: Nd  M,Np  /re/d  y/abc/xyz/)".into());
        }
        rest.remove(0).to_string()
    } else {
        scripts.join("\n")
    };
    let quiet = a.has('n');
    let cmds = parse_script(&script, a.has('E') || a.has('r'))?;
    let text = one_input(&rest, stdin, read)?;
    let lines = lines_of(&text);
    let mut out: Vec<String> = Vec::new();
    // a range that has started: index of the command → still open
    let mut open = vec![false; cmds.len()];
    'lines: for (i, l) in lines.iter().enumerate() {
        let n = i + 1;
        let last = n == lines.len();
        let mut cur = l.to_string();
        let mut deleted = false;
        for (ci, c) in cmds.iter().enumerate() {
            let selected = match (&c.a1, &c.a2) {
                (None, _) => true,
                (Some(a), None) => addr_hit(a, n, last, &cur),
                (Some(a), Some(b)) => {
                    if open[ci] {
                        if addr_hit(b, n, last, &cur) || matches!(b, Addr::Line(e) if *e <= n) {
                            open[ci] = false;
                        }
                        true
                    } else if addr_hit(a, n, last, &cur) {
                        open[ci] = !(matches!(b, Addr::Line(e) if *e <= n) || (matches!(b, Addr::Last) && last));
                        true
                    } else {
                        false
                    }
                }
            } != c.negate;
            if !selected {
                continue;
            }
            match &c.act {
                Act::Delete => {
                    deleted = true;
                    break;
                }
                Act::Print => out.push(cur.clone()),
                Act::Quit => {
                    if !quiet {
                        out.push(cur);
                    }
                    break 'lines;
                }
                Act::Translate(from, to) => cur = cur.chars().map(|ch| from.iter().position(|f| *f == ch).map_or(ch, |p| to[p])).collect(),
                Act::Sub { re, repl, global, nth, print } => {
                    let chars: Vec<char> = cur.chars().collect();
                    let (mut res, mut at, mut count, mut copied) = (String::new(), 0usize, 0usize, 0usize);
                    let mut changed = false;
                    while at <= chars.len() {
                        let Some((s, e, caps)) = re.find_at(&chars, at) else { break };
                        count += 1;
                        if count >= nth.max(&1).to_owned() {
                            res.extend(&chars[copied..s]);
                            for p in repl {
                                match p {
                                    ReplPart::Lit(t) => res.push_str(t),
                                    ReplPart::Group(g) => {
                                        if let Some(Some((a, b))) = caps.get(*g) {
                                            res.extend(&chars[*a..*b]);
                                        }
                                    }
                                }
                            }
                            copied = e;
                            changed = true;
                            if !*global {
                                break;
                            }
                        }
                        at = if e > s { e } else { e + 1 };
                        if e == s && s < chars.len() && count >= nth.max(&1).to_owned() {
                            res.push(chars[s]);
                            copied = s + 1;
                        }
                    }
                    if changed {
                        res.extend(&chars[copied.min(chars.len())..]);
                        cur = res;
                        if *print {
                            out.push(cur.clone());
                        }
                    }
                }
            }
        }
        if !deleted && !quiet {
            out.push(cur);
        }
    }
    Ok(join(&out))
}

// ---------------------------------------------------------------------------------------------------------- tr, cut

fn expand_set(s: &str) -> Result<Vec<char>, String> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '[' && c.get(i + 1) == Some(&':') {
            if let Some(e) = c[i..].iter().collect::<String>().find(":]") {
                let name: String = c[i + 2..i + e].iter().collect();
                let set: Vec<char> = match name.as_str() {
                    "upper" => ('A'..='Z').collect(),
                    "lower" => ('a'..='z').collect(),
                    "digit" => ('0'..='9').collect(),
                    "alpha" => ('a'..='z').chain('A'..='Z').collect(),
                    "alnum" => ('a'..='z').chain('A'..='Z').chain('0'..='9').collect(),
                    "space" => vec![' ', '\t', '\n', '\r', '\x0b', '\x0c'],
                    "blank" => vec![' ', '\t'],
                    "punct" => ('!'..='/').chain(':'..='@').chain('['..='`').chain('{'..='~').collect(),
                    other => return Err(format!("unknown class [:{other}:]")),
                };
                out.extend(set);
                i += e + 2;
                continue;
            }
        }
        let ch = if c[i] == '\\' && i + 1 < c.len() {
            i += 1;
            match c[i] { 'n' => '\n', 't' => '\t', 'r' => '\r', o => o }
        } else {
            c[i]
        };
        if c.get(i + 1) == Some(&'-') && i + 2 < c.len() {
            let hi = c[i + 2];
            if hi < ch {
                return Err("a range runs backwards".into());
            }
            out.extend(ch..=hi);
            i += 3;
        } else {
            out.push(ch);
            i += 1;
        }
    }
    Ok(out)
}

fn tr(args: &[String], stdin: Option<&str>) -> Result<String, String> {
    let a = parse(args, "")?;
    a.unknown("dsc")?;
    let (delete, squeeze, complement) = (a.has('d'), a.has('s'), a.has('c'));
    let text = stdin.ok_or("tr works on a pipe: `echo hello | tr a-z A-Z`")?;
    let set1 = expand_set(a.rest.first().ok_or("usage: tr [-d] [-s] [-c] SET1 [SET2]")?)?;
    let set2 = a.rest.get(1).map(|s| expand_set(s)).transpose()?;
    let in1 = |c: char| set1.contains(&c) != complement;
    let mut out = String::new();
    let mut last: Option<char> = None;
    for ch in text.chars() {
        let mapped = if delete && in1(ch) {
            None
        } else if let (Some(s2), false) = (&set2, delete) {
            if in1(ch) {
                if complement { s2.last().copied() } else { set1.iter().position(|c| *c == ch).map(|p| s2.get(p).or(s2.last()).copied().unwrap_or(ch)) }
            } else {
                Some(ch)
            }
        } else {
            Some(ch)
        };
        let Some(m) = mapped else { continue };
        let in_target = if delete { false } else { set2.as_ref().map_or(in1(ch), |s2| s2.contains(&m) && in1(ch)) };
        if squeeze && in_target && last == Some(m) {
            continue;
        }
        out.push(m);
        last = Some(m);
    }
    Ok(out)
}

fn ranges(list: &str) -> Result<Vec<(usize, usize)>, String> {
    list.split(',')
        .map(|p| {
            let n = |s: &str| s.parse::<usize>().ok().filter(|v| *v >= 1);
            match p.split_once('-') {
                None => n(p).map(|v| (v, v)),
                Some(("", b)) => n(b).map(|b| (1, b)),
                Some((a, "")) => n(a).map(|a| (a, usize::MAX)),
                Some((a, b)) => n(a).zip(n(b)).filter(|(a, b)| a <= b),
            }
            .ok_or_else(|| format!("bad list `{list}` (fields count from 1, like 1,3-5 or 2-)"))
        })
        .collect()
}

fn cut(args: &[String], stdin: Option<&str>, read: Reader) -> Result<String, String> {
    let a = parse(args, "dfcb")?;
    a.unknown("s")?;
    let text = one_input(&a.rest, stdin, read)?;
    let (fields, chars) = (a.opt('f'), a.opt('c').or(a.opt('b')));
    let delim = a.opt('d').map_or(Ok('\t'), |d| if d.chars().count() == 1 { Ok(d.chars().next().unwrap_or('\t')) } else { Err("the delimiter must be one character") })?;
    let mut out = Vec::new();
    for line in lines_of(&text) {
        if let Some(list) = fields {
            let r = ranges(list)?;
            if !line.contains(delim) {
                if !a.has('s') {
                    out.push(line.to_string());
                }
                continue;
            }
            let parts: Vec<&str> = line.split(delim).collect();
            let picked: Vec<&str> = parts.iter().enumerate().filter(|(i, _)| r.iter().any(|(lo, hi)| (*lo..=*hi).contains(&(i + 1)))).map(|(_, p)| *p).collect();
            out.push(picked.join(&delim.to_string()));
        } else if let Some(list) = chars {
            let r = ranges(list)?;
            out.push(line.chars().enumerate().filter(|(i, _)| r.iter().any(|(lo, hi)| (*lo..=*hi).contains(&(i + 1)))).map(|(_, c)| c).collect());
        } else {
            return Err("usage: cut -d, -f1,3   or   cut -c1-5".into());
        }
    }
    Ok(join(&out))
}

// -------------------------------------------------------------------------------------- sort, uniq, wc, head, tail

fn leading_number(s: &str) -> f64 {
    let t = s.trim_start();
    let end = t.char_indices().find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+')))).map_or(t.len(), |(i, _)| i);
    t[..end].parse().unwrap_or(0.0)
}

fn sort(args: &[String], stdin: Option<&str>, read: Reader) -> Result<String, String> {
    let a = parse(args, "kt")?;
    a.unknown("rnufb")?;
    let text = one_input(&a.rest, stdin, read)?;
    let mut lines: Vec<String> = lines_of(&text).iter().map(|s| s.to_string()).collect();
    let key = a.opt('k').map(|k| k.split(',').next().unwrap_or("1").parse::<usize>().ok().filter(|n| *n >= 1).ok_or("-k needs a field number from 1")).transpose()?;
    let sep = a.opt('t').and_then(|t| t.chars().next());
    let keyed = |l: &str| -> String {
        match key {
            None => l.to_string(),
            Some(k) => match sep {
                Some(s) => l.split(s).nth(k - 1).unwrap_or("").to_string(),
                None => l.split_whitespace().nth(k - 1).unwrap_or("").to_string(),
            },
        }
    };
    let fold = a.has('f');
    lines.sort_by(|x, y| {
        let (kx, ky) = (keyed(x), keyed(y));
        let o = if a.has('n') { leading_number(&kx).partial_cmp(&leading_number(&ky)).unwrap_or(std::cmp::Ordering::Equal) } else if fold { kx.to_lowercase().cmp(&ky.to_lowercase()) } else { kx.cmp(&ky) };
        o.then_with(|| x.cmp(y))
    });
    if a.has('r') {
        lines.reverse();
    }
    if a.has('u') {
        lines.dedup_by(|x, y| if a.has('n') { leading_number(&keyed(x)) == leading_number(&keyed(y)) } else { keyed(x) == keyed(y) });
    }
    Ok(join(&lines))
}

fn uniq(args: &[String], stdin: Option<&str>, read: Reader) -> Result<String, String> {
    let a = parse(args, "")?;
    a.unknown("cdui")?;
    let text = one_input(&a.rest, stdin, read)?;
    let key = |l: &str| if a.has('i') { l.to_lowercase() } else { l.to_string() };
    let mut groups: Vec<(String, usize)> = Vec::new();
    for l in lines_of(&text) {
        match groups.last_mut() {
            Some((p, n)) if key(p) == key(l) => *n += 1,
            _ => groups.push((l.to_string(), 1)),
        }
    }
    let out: Vec<String> = groups.into_iter().filter(|(_, n)| (!a.has('d') || *n > 1) && (!a.has('u') || *n == 1)).map(|(l, n)| if a.has('c') { format!("{n:>7} {l}") } else { l }).collect();
    Ok(join(&out))
}

fn wc(args: &[String], stdin: Option<&str>, read: Reader) -> Result<String, String> {
    let a = parse(args, "")?;
    a.unknown("lwcm")?;
    let ins = inputs(&a.rest, stdin, read)?;
    let (l, w, c) = (a.has('l'), a.has('w'), a.has('c') || a.has('m'));
    let all = !(l || w || c);
    let mut rows = Vec::new();
    let mut tot = (0, 0, 0);
    for (name, t) in &ins {
        let counts = (t.matches('\n').count(), t.split_whitespace().count(), if a.has('m') { t.chars().count() } else { t.len() });
        tot = (tot.0 + counts.0, tot.1 + counts.1, tot.2 + counts.2);
        let mut cols = Vec::new();
        if l || all {
            cols.push(format!("{:>7}", counts.0));
        }
        if w || all {
            cols.push(format!("{:>7}", counts.1));
        }
        if c || all {
            cols.push(format!("{:>7}", counts.2));
        }
        rows.push(format!("{}{}", cols.join(" "), if a.rest.is_empty() { String::new() } else { format!(" {name}") }));
    }
    if ins.len() > 1 {
        let mut cols = Vec::new();
        if l || all {
            cols.push(format!("{:>7}", tot.0));
        }
        if w || all {
            cols.push(format!("{:>7}", tot.1));
        }
        if c || all {
            cols.push(format!("{:>7}", tot.2));
        }
        rows.push(format!("{} total", cols.join(" ")));
    }
    Ok(join(&rows))
}

fn head_tail(head: bool, args: &[String], stdin: Option<&str>, read: Reader) -> Result<String, String> {
    let a = parse(args, "nc")?;
    a.unknown("")?;
    let text = one_input(&a.rest, stdin, read)?;
    if let Some(c) = a.opt('c') {
        let n: usize = c.parse().map_err(|_| "-c needs a number")?;
        let chars: Vec<char> = text.chars().collect();
        return Ok(if head { chars.iter().take(n).collect() } else { chars[chars.len().saturating_sub(n)..].iter().collect() });
    }
    let n = a.num('n', 10)?;
    let lines = lines_of(&text);
    let pick: Vec<String> = if head { lines.iter().take(n).map(|s| s.to_string()).collect() } else { lines[lines.len().saturating_sub(n)..].iter().map(|s| s.to_string()).collect() };
    Ok(join(&pick))
}

// ------------------------------------------------------------------------------------------- seq, date, basename

fn seq(args: &[String]) -> Result<String, String> {
    let n: Vec<f64> = args.iter().map(|s| s.parse::<f64>().map_err(|_| format!("`{s}` is not a number"))).collect::<Result<_, _>>()?;
    let (from, step, to) = match n.as_slice() {
        [t] => (1.0, 1.0, *t),
        [f, t] => (*f, 1.0, *t),
        [f, s, t] => (*f, *s, *t),
        _ => return Err("usage: seq LAST | FIRST LAST | FIRST STEP LAST".into()),
    };
    if step == 0.0 || (to - from) / step > 100_000.0 {
        return Err("too many numbers (at most 100000) or a step of 0".into());
    }
    let mut out = Vec::new();
    let mut v = from;
    while (step > 0.0 && v <= to + 1e-9) || (step < 0.0 && v >= to - 1e-9) {
        out.push(if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v}") });
        v += step;
    }
    Ok(join(&out))
}

fn date(args: &[String]) -> Result<String, String> {
    let mut utc = false;
    let mut fmt = None;
    let mut at = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-u" => utc = true,
            "-d" | "--date" => {
                i += 1;
                let v = args.get(i).ok_or("-d needs a value like @1700000000")?;
                at = Some(v.strip_prefix('@').and_then(|n| n.parse::<i64>().ok()).ok_or("-d understands only @UNIXSECONDS")?);
            }
            f if f.starts_with('+') => fmt = Some(f[1..].to_string()),
            o => return Err(format!("`{o}` is not supported (use: date [-u] [+FORMAT])")),
        }
        i += 1;
    }
    let zone = if utc { clock::Zone::utc() } else { clock::Zone::system() };
    let t = at.unwrap_or_else(clock::now_unix);
    Ok(format!("{}\n", clock::format(&zone, t, &fmt.unwrap_or_else(|| "%a %b %e %H:%M:%S %Z %Y".into()))))
}

fn path_part(base: bool, args: &[String]) -> Result<String, String> {
    let p = args.first().ok_or("needs a path")?.trim_end_matches('/');
    let (dir, name) = match p.rfind('/') {
        Some(0) => ("/", &p[1..]),
        Some(i) => (&p[..i], &p[i + 1..]),
        None => (".", p),
    };
    let mut r = if base { name.to_string() } else { dir.to_string() };
    if base {
        if let Some(suffix) = args.get(1) {
            if r.len() > suffix.len() && r.ends_with(suffix.as_str()) {
                r.truncate(r.len() - suffix.len());
            }
        }
    }
    Ok(format!("{r}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn no_files(_: &str) -> Result<String, String> {
        Err("no such file".into())
    }

    fn go(cmd: &str, a: &[&str], stdin: &str) -> String {
        match run(cmd, &v(a), Some(stdin), &no_files) {
            Some(Ok(s)) => s,
            Some(Err(e)) => format!("ERR {e}"),
            None => "NOT A TEXT COMMAND".into(),
        }
    }

    #[test]
    fn grep_finds_lines_counts_numbers_inverts_and_prints_only_the_match() {
        let t = "apple\nBanana\ncherry\napple pie\n";
        assert_eq!(go("grep", &["apple"], t), "apple\napple pie\n");
        assert_eq!(go("grep", &["-i", "banana"], t), "Banana\n");
        assert_eq!(go("grep", &["-v", "apple"], t), "Banana\ncherry\n");
        assert_eq!(go("grep", &["-c", "an"], t), "1\n");
        assert_eq!(go("grep", &["-n", "pie"], t), "4:apple pie\n");
        assert_eq!(go("grep", &["-o", "-E", "[a-z]+e"], "one two three"), "one\nthree\n");
        assert_eq!(go("grep", &["-w", "app"], t), "");
        assert_eq!(go("grep", &["-x", "apple"], t), "apple\n");
        assert_eq!(go("grep", &["-F", "a.b"], "a.b\naxb\n"), "a.b\n");
        assert_eq!(go("grep", &["-m", "1", "a"], t), "apple\n");
        assert_eq!(go("grep", &["-e", "x", "-e", "y"], "x\ny\nz\n"), "x\ny\n", "several -e patterns are alternatives");
        assert!(go("grep", &["(", "-E"], t).starts_with("ERR bad pattern"));
        assert!(go("grep", &[], t).starts_with("ERR usage"));
        assert_eq!(go("grep", &["a\\|c"], "a\nb\nc\n"), "a\nc\n", "basic syntax alternation");
    }

    #[test]
    fn sed_substitutes_deletes_prints_ranges_and_translates() {
        assert_eq!(go("sed", &["s/world/there/"], "hello world\nworld world\n"), "hello there\nthere world\n");
        assert_eq!(go("sed", &["s/o/0/g"], "foo boo\n"), "f00 b00\n");
        assert_eq!(go("sed", &["s/o/0/2"], "foo boo\n"), "fo0 boo\n");
        assert_eq!(go("sed", &["-E", "s/(\\w+) (\\w+)/\\2 \\1/"], "hello world\n"), "world hello\n");
        assert_eq!(go("sed", &["s|/usr|/opt|"], "/usr/bin\n"), "/opt/bin\n");
        assert_eq!(go("sed", &["s/a/[&]/g"], "banana\n"), "b[a]n[a]n[a]\n");
        assert_eq!(go("sed", &["s/x*/-/g"], "abc\n"), "-a-b-c-\n");
        assert_eq!(go("sed", &["2d"], "a\nb\nc\n"), "a\nc\n");
        assert_eq!(go("sed", &["$d"], "a\nb\nc\n"), "a\nb\n");
        assert_eq!(go("sed", &["-n", "2,3p"], "a\nb\nc\nd\n"), "b\nc\n");
        assert_eq!(go("sed", &["/b/,/c/d"], "a\nb\nx\nc\nd\n"), "a\nd\n");
        assert_eq!(go("sed", &["-n", "/err/p"], "ok\nerror 1\nok\n"), "error 1\n");
        assert_eq!(go("sed", &["/ok/!d"], "ok\nbad\nok\n"), "ok\nok\n");
        assert_eq!(go("sed", &["y/abc/xyz/"], "aabbcc\n"), "xxyyzz\n");
        assert_eq!(go("sed", &["-e", "s/a/b/", "-e", "s/b/c/"], "a\n"), "c\n");
        assert_eq!(go("sed", &["s/a/b/;s/c/d/"], "ac\n"), "bd\n");
        assert_eq!(go("sed", &["2q"], "a\nb\nc\n"), "a\nb\n");
        assert_eq!(go("sed", &["s/HELLO/bye/i"], "Hello\n"), "bye\n");
        assert_eq!(go("sed", &["s/a/\\n/"], "xay\n"), "x\ny\n");
        for bad in [&["s/a/b"][..], &["s/\\(/x/"], &["k"], &["s/a/\\3/"], &["3"], &["y/ab/x/"]] {
            assert!(go("sed", bad, "a\n").starts_with("ERR"), "{bad:?}");
        }
        assert!(go("sed", &["-i", "s/a/b/", "f"], "a\n").starts_with("ERR -i is not supported"), "nothing is edited in place");
    }

    #[test]
    fn tr_cut_sort_uniq_wc_head_tail_and_friends() {
        assert_eq!(go("tr", &["a-z", "A-Z"], "Hello, World\n"), "HELLO, WORLD\n");
        assert_eq!(go("tr", &["[:lower:]", "[:upper:]"], "abc"), "ABC");
        assert_eq!(go("tr", &["-d", "0-9"], "a1b22c"), "abc");
        assert_eq!(go("tr", &["-s", " "], "a   b    c"), "a b c");
        assert_eq!(go("tr", &["-d", "\\n"], "a\nb\n"), "ab");
        assert_eq!(go("tr", &["-cd", "a-z"], "a1b2\n"), "ab");
        assert_eq!(go("tr", &[",", "\\n"], "a,b,c"), "a\nb\nc");
        assert!(go("tr", &["z-a", "x"], "q").starts_with("ERR"));
        assert_eq!(go("cut", &["-d,", "-f2"], "a,b,c\nd,e,f\n"), "b\ne\n");
        assert_eq!(go("cut", &["-d", ":", "-f1,3"], "a:b:c\n"), "a:c\n");
        assert_eq!(go("cut", &["-f2-"], "a\tb\tc\n"), "b\tc\n");
        assert_eq!(go("cut", &["-c1-3"], "abcdef\n"), "abc\n");
        assert_eq!(go("cut", &["-c", "-2"], "abcdef\n"), "ab\n");
        assert!(go("cut", &["-f0"], "a\n").starts_with("ERR bad list"));
        assert_eq!(go("sort", &[], "b\na\nc\n"), "a\nb\nc\n");
        assert_eq!(go("sort", &["-r"], "b\na\nc\n"), "c\nb\na\n");
        assert_eq!(go("sort", &["-n"], "10\n9\n100\n"), "9\n10\n100\n");
        assert_eq!(go("sort", &["-u"], "a\nb\na\n"), "a\nb\n");
        assert_eq!(go("sort", &["-t,", "-k2", "-n"], "x,3\ny,1\nz,2\n"), "y,1\nz,2\nx,3\n");
        assert_eq!(go("uniq", &["-c"], "a\na\nb\n"), "      2 a\n      1 b\n");
        assert_eq!(go("uniq", &["-d"], "a\na\nb\n"), "a\n");
        assert_eq!(go("wc", &["-l"], "a\nb\n"), "      2\n");
        assert_eq!(go("wc", &[], "a b\nc\n"), "      2       3       6\n");
        assert_eq!(go("head", &["-n", "2"], "1\n2\n3\n"), "1\n2\n");
        assert_eq!(go("head", &["-1"], "1\n2\n"), "1\n");
        assert_eq!(go("tail", &["-n2"], "1\n2\n3\n"), "2\n3\n");
        assert_eq!(go("tac", &[], "1\n2\n"), "2\n1\n");
        assert_eq!(go("rev", &[], "abc\n"), "cba\n");
        assert_eq!(go("nl", &[], "a\nb\n"), "     1\ta\n     2\tb\n");
        assert_eq!(go("seq", &["3"], ""), "1\n2\n3\n");
        assert_eq!(go("seq", &["2", "2", "8"], ""), "2\n4\n6\n8\n");
        assert_eq!(go("seq", &["5", "-2", "1"], ""), "5\n3\n1\n");
        assert!(go("seq", &["1", "0", "5"], "").starts_with("ERR"));
        assert_eq!(go("basename", &["/a/b/c.txt", ".txt"], ""), "c\n");
        assert_eq!(go("dirname", &["/a/b/c.txt"], ""), "/a/b\n");
        assert_eq!(go("dirname", &["c.txt"], ""), ".\n");
        assert_eq!(go("nosuch", &[], ""), "NOT A TEXT COMMAND");
    }

    #[test]
    fn date_prints_the_time_in_a_chosen_format_and_files_come_through_the_reader() {
        assert_eq!(go("date", &["-u", "-d", "@1000000000", "+%F %T %A"], ""), "2001-09-09 01:46:40 Sunday\n");
        assert_eq!(go("date", &["-u", "-d", "@0"], ""), "Thu Jan  1 00:00:00 UTC 1970\n");
        assert!(go("date", &["+%Y"], "").trim().parse::<i32>().unwrap() >= 2025);
        assert!(go("date", &["--set=x"], "").starts_with("ERR"));
        let read = |f: &str| if f == "a.txt" { Ok("one\ntwo\n".to_string()) } else { Err(format!("{f}: no such file")) };
        assert_eq!(run("grep", &v(&["t", "a.txt"]), None, &read).unwrap().unwrap(), "two\n");
        assert_eq!(run("grep", &v(&["o", "a.txt", "a.txt"]), None, &read).unwrap().unwrap(), "a.txt:one\na.txt:two\na.txt:one\na.txt:two\n");
        assert!(run("grep", &v(&["o", "b.txt"]), None, &read).unwrap().unwrap_err().contains("no such file"));
        assert!(run("sort", &[], None, &read).unwrap().unwrap_err().contains("needs a file or a pipe"));
    }
}
