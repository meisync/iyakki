//! PDF objects and the tokenizer/parser shared by file structure and content streams. Hostile input: nesting depth is capped,
//! numbers never overflow, and every read is bounds-checked.

use std::collections::BTreeMap;
use std::rc::Rc;

pub type Dict = BTreeMap<String, Obj>;

pub const MAX_DEPTH: usize = 48;
const MAX_ITEMS: usize = 2_000_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Str(Rc<Vec<u8>>),
    Name(Rc<str>),
    Arr(Rc<Vec<Obj>>),
    Dict(Rc<Dict>),
    /// A stream: its dictionary and where its raw bytes lie in the file (`start..end`).
    Stream(Rc<Dict>, usize, usize),
    Ref(u32, u16),
}

impl Obj {
    pub fn num(&self) -> Option<f64> {
        match self {
            Obj::Int(i) => Some(*i as f64),
            Obj::Real(r) if r.is_finite() => Some(*r),
            _ => None,
        }
    }
    pub fn int(&self) -> Option<i64> {
        match self {
            Obj::Int(i) => Some(*i),
            Obj::Real(r) if r.is_finite() => Some(*r as i64),
            _ => None,
        }
    }
    pub fn name(&self) -> Option<&str> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }
    pub fn arr(&self) -> Option<&Rc<Vec<Obj>>> {
        match self {
            Obj::Arr(a) => Some(a),
            _ => None,
        }
    }
    /// The dictionary of a dictionary or a stream.
    pub fn dict(&self) -> Option<&Rc<Dict>> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _, _) => Some(d),
            _ => None,
        }
    }
    pub fn str_bytes(&self) -> Option<&[u8]> {
        match self {
            Obj::Str(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Obj(Obj),
    Keyword(String),
    ArrOpen,
    ArrClose,
    DictOpen,
    DictClose,
    Eof,
}

pub fn is_ws(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}
pub fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}
fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub struct Lexer<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Lexer<'a> {
        Lexer { data, pos }
    }

    pub fn skip_ws(&mut self) {
        while let Some(&b) = self.data.get(self.pos) {
            if is_ws(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(&c) = self.data.get(self.pos) {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    pub fn next(&mut self) -> Tok {
        loop {
            if let Some(t) = self.next_inner() {
                return t;
            }
        }
    }

    /// `None`: a stray delimiter was skipped, look again.
    fn next_inner(&mut self) -> Option<Tok> {
        self.skip_ws();
        let Some(&b) = self.data.get(self.pos) else { return Some(Tok::Eof) };
        Some(match b {
            b'[' => {
                self.pos += 1;
                Tok::ArrOpen
            }
            b']' => {
                self.pos += 1;
                Tok::ArrClose
            }
            b'<' => {
                if self.data.get(self.pos + 1) == Some(&b'<') {
                    self.pos += 2;
                    Tok::DictOpen
                } else {
                    self.pos += 1;
                    Tok::Obj(Obj::Str(Rc::new(self.hex_string())))
                }
            }
            b'>' => {
                if self.data.get(self.pos + 1) == Some(&b'>') {
                    self.pos += 2;
                    Tok::DictClose
                } else {
                    self.pos += 1;
                    return None;
                }
            }
            b'(' => {
                self.pos += 1;
                Tok::Obj(Obj::Str(Rc::new(self.lit_string())))
            }
            b')' | b'{' | b'}' => {
                self.pos += 1;
                return None;
            }
            b'/' => {
                self.pos += 1;
                Tok::Obj(Obj::Name(self.name().into()))
            }
            _ => {
                let start = self.pos;
                while let Some(&c) = self.data.get(self.pos) {
                    if is_ws(c) || is_delim(c) {
                        break;
                    }
                    self.pos += 1;
                }
                let word = &self.data[start..self.pos];
                if let Some(n) = parse_number(word) {
                    return Some(Tok::Obj(n));
                }
                match word {
                    b"true" => Tok::Obj(Obj::Bool(true)),
                    b"false" => Tok::Obj(Obj::Bool(false)),
                    b"null" => Tok::Obj(Obj::Null),
                    _ => Tok::Keyword(String::from_utf8_lossy(&word[..word.len().min(64)]).into_owned()),
                }
            }
        })
    }

    fn name(&mut self) -> String {
        let mut out = Vec::new();
        while let Some(&c) = self.data.get(self.pos) {
            if is_ws(c) || is_delim(c) {
                break;
            }
            self.pos += 1;
            if c == b'#' {
                if let (Some(h), Some(l)) = (self.data.get(self.pos).and_then(|b| hexval(*b)), self.data.get(self.pos + 1).and_then(|b| hexval(*b))) {
                    out.push(h * 16 + l);
                    self.pos += 2;
                    continue;
                }
            }
            out.push(c);
            if out.len() >= 512 {
                break;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn hex_string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        while let Some(&c) = self.data.get(self.pos) {
            self.pos += 1;
            if c == b'>' {
                break;
            }
            if let Some(v) = hexval(c) {
                match hi.take() {
                    Some(h) => out.push(h * 16 + v),
                    None => hi = Some(v),
                }
            }
        }
        if let Some(h) = hi {
            out.push(h * 16);
        }
        out
    }

    fn lit_string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut depth = 1usize;
        while let Some(&c) = self.data.get(self.pos) {
            self.pos += 1;
            match c {
                b'(' => {
                    depth += 1;
                    out.push(c);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(c);
                }
                b'\\' => {
                    let Some(&e) = self.data.get(self.pos) else { break };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.data.get(self.pos) {
                                    Some(&d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((v & 0xff) as u8);
                        }
                        b'\r' => {
                            if self.data.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                _ => out.push(c),
            }
            if out.len() > 16 * 1024 * 1024 {
                break;
            }
        }
        out
    }
}

pub fn parse_number(w: &[u8]) -> Option<Obj> {
    if w.is_empty() || w.len() > 40 {
        return None;
    }
    let mut i = 0;
    let mut neg = false;
    while i < w.len() && (w[i] == b'+' || w[i] == b'-') {
        neg = neg || w[i] == b'-';
        i += 1;
    }
    if i >= w.len() {
        return None;
    }
    let mut int: i64 = 0;
    let mut frac = 0f64;
    let mut scale = 1f64;
    let mut dot = false;
    let mut digits = 0;
    let mut overflow = false;
    for &c in &w[i..] {
        match c {
            b'0'..=b'9' => {
                digits += 1;
                if dot {
                    if scale < 1e15 {
                        scale *= 10.0;
                        frac += (c - b'0') as f64 / scale;
                    }
                } else {
                    match int.checked_mul(10).and_then(|v| v.checked_add((c - b'0') as i64)) {
                        Some(v) => int = v,
                        None => overflow = true,
                    }
                }
            }
            b'.' if !dot => dot = true,
            b'-' => {} // "12-3" and other damaged numbers: the stray sign is dropped
            _ => return None,
        }
    }
    if digits == 0 {
        return None;
    }
    if overflow {
        return Some(Obj::Real(if neg { -1e18 } else { 1e18 }));
    }
    if dot {
        let v = int as f64 + frac;
        Some(Obj::Real(if neg { -v } else { v }))
    } else {
        Some(Obj::Int(if neg { -int } else { int }))
    }
}

/// Parse one object starting at the lexer position. Returns the keyword when a bare keyword comes first (content operators,
/// `endobj`, `stream`). `refs`: recognise `N G R`.
pub fn parse_obj(lx: &mut Lexer, refs: bool, depth: usize) -> Result<Tok, String> {
    if depth > MAX_DEPTH {
        return Err("objects nested too deeply".into());
    }
    let t = lx.next();
    match t {
        Tok::Obj(Obj::Int(n)) if refs && n >= 0 && n <= u32::MAX as i64 => {
            let save = lx.pos;
            if let Tok::Obj(Obj::Int(g)) = lx.next() {
                if (0..=65535).contains(&g) {
                    if let Tok::Keyword(k) = lx.next() {
                        if k == "R" {
                            return Ok(Tok::Obj(Obj::Ref(n as u32, g as u16)));
                        }
                    }
                }
            }
            lx.pos = save;
            Ok(Tok::Obj(Obj::Int(n)))
        }
        Tok::ArrOpen => {
            let mut v = Vec::new();
            loop {
                match parse_obj(lx, refs, depth + 1)? {
                    Tok::ArrClose | Tok::Eof => break,
                    Tok::Obj(o) => v.push(o),
                    Tok::Keyword(_) | Tok::DictClose | Tok::ArrOpen | Tok::DictOpen => {} // stray token inside an array: skipped
                }
                if v.len() > MAX_ITEMS {
                    return Err("array too large".into());
                }
            }
            Ok(Tok::Obj(Obj::Arr(Rc::new(v))))
        }
        Tok::DictOpen => {
            let mut d = Dict::new();
            loop {
                match parse_obj(lx, refs, depth + 1)? {
                    Tok::DictClose | Tok::Eof => break,
                    Tok::Obj(Obj::Name(k)) => match parse_obj(lx, refs, depth + 1)? {
                        Tok::Obj(v) => {
                            d.insert(k.to_string(), v);
                        }
                        Tok::DictClose | Tok::Eof => break,
                        _ => {}
                    },
                    _ => {}
                }
                if d.len() > MAX_ITEMS {
                    return Err("dictionary too large".into());
                }
            }
            Ok(Tok::Obj(Obj::Dict(Rc::new(d))))
        }
        other => Ok(other),
    }
}

/// Parse a standalone object from a byte slice (used by object streams and tests).
pub fn parse_value(data: &[u8], pos: usize) -> Option<Obj> {
    let mut lx = Lexer::new(data, pos);
    match parse_obj(&mut lx, true, 0) {
        Ok(Tok::Obj(o)) => Some(o),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_names_strings_and_refs() {
        let o = parse_value(b"<< /A 12 /B -3.5 /C (a\\(b\\)\\101) /D <4142> /E [1 0 R /N#20x] /F true >>", 0).unwrap();
        let d = o.dict().unwrap();
        assert_eq!(d["A"], Obj::Int(12));
        assert_eq!(d["B"], Obj::Real(-3.5));
        assert_eq!(d["C"].str_bytes().unwrap(), b"a(b)A");
        assert_eq!(d["D"].str_bytes().unwrap(), b"AB");
        let e = d["E"].arr().unwrap();
        assert_eq!(e[0], Obj::Ref(1, 0));
        assert_eq!(e[1].name().unwrap(), "N x");
        assert_eq!(d["F"], Obj::Bool(true));
    }

    #[test]
    fn deep_nesting_is_refused_not_overflowed() {
        let mut s = vec![b'['; 100_000];
        s.extend(vec![b']'; 100_000]);
        let mut lx = Lexer::new(&s, 0);
        assert!(parse_obj(&mut lx, true, 0).is_err());
    }

    #[test]
    fn damaged_numbers_do_not_panic() {
        assert_eq!(parse_number(b"99999999999999999999999"), Some(Obj::Real(1e18)));
        assert_eq!(parse_number(b"--5"), Some(Obj::Int(-5)));
        assert_eq!(parse_number(b"."), None);
        assert_eq!(parse_number(b"1.2.3"), None);
    }
}
