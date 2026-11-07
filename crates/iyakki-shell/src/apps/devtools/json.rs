//! A small JSON reader and writer for the Developer tools: parses once into a tree (numbers keep their text, so nothing is
//! rounded), reports the first error as line and column, and prints it pretty, minified or with the keys sorted.

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// The number as written.
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    /// Members in the order written (a repeated key stays).
    Obj(Vec<(String, Json)>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub msg: String,
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at line {}, column {}", self.msg, self.line, self.col)
    }
}

/// How deep arrays and objects may nest.
const MAX_DEPTH: usize = 400;

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

pub fn parse(s: &str) -> Result<Json, JsonError> {
    let mut p = P { b: s.as_bytes(), i: 0 };
    p.ws();
    let v = p.value(0).map_err(|(m, at)| locate(s, at, m))?;
    p.ws();
    if p.i < p.b.len() {
        return Err(locate(s, p.i, "Unexpected text after the value".into()));
    }
    Ok(v)
}

/// Line and column (counted in characters, from 1) of byte offset `at`.
fn locate(s: &str, at: usize, msg: String) -> JsonError {
    let at = at.min(s.len());
    let head = &s.as_bytes()[..at];
    let line = head.iter().filter(|b| **b == b'\n').count() + 1;
    let start = head.iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
    let col = s.get(start..at).map_or(at - start, |t| t.chars().count()) + 1;
    JsonError { msg, line, col }
}

type PR<T> = Result<T, (String, usize)>;

impl P<'_> {
    fn ws(&mut self) {
        while matches!(self.b.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }
    fn err<T>(&self, m: &str) -> PR<T> {
        Err((m.to_string(), self.i))
    }
    fn lit(&mut self, word: &str, v: Json) -> PR<Json> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            self.err("Unexpected character")
        }
    }
    fn value(&mut self, depth: usize) -> PR<Json> {
        if depth > MAX_DEPTH {
            return self.err("Nested too deeply");
        }
        match self.b.get(self.i) {
            None => self.err("Unexpected end of input"),
            Some(b'{') => {
                self.i += 1;
                let mut m = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    if self.b.get(self.i) != Some(&b'"') {
                        return self.err("Expected a string key");
                    }
                    let k = self.string()?;
                    self.ws();
                    if self.b.get(self.i) != Some(&b':') {
                        return self.err("Expected ':'");
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value(depth + 1)?;
                    m.push((k, v));
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(m));
                        }
                        _ => return self.err("Expected ',' or '}'"),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    self.ws();
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(a));
                        }
                        _ => return self.err("Expected ',' or ']'"),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => self.err("Unexpected character"),
        }
    }
    fn number(&mut self) -> PR<Json> {
        let st = self.i;
        if self.b[self.i] == b'-' {
            self.i += 1;
        }
        let digits = |p: &mut Self| {
            let s = p.i;
            while matches!(p.b.get(p.i), Some(b'0'..=b'9')) {
                p.i += 1;
            }
            p.i - s
        };
        if digits(self) == 0 {
            return self.err("Expected a digit");
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            if digits(self) == 0 {
                return self.err("Expected a digit after '.'");
            }
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if digits(self) == 0 {
                return self.err("Expected a digit in the exponent");
            }
        }
        Ok(Json::Num(String::from_utf8_lossy(&self.b[st..self.i]).into_owned()))
    }
    fn hex4(&mut self) -> PR<u32> {
        let h = self.b.get(self.i..self.i + 4).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u32::from_str_radix(h, 16).ok());
        match h {
            Some(v) => {
                self.i += 4;
                Ok(v)
            }
            None => self.err("Expected four hex digits"),
        }
    }
    fn string(&mut self) -> PR<String> {
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            // copy a plain run at once
            let st = self.i;
            while self.i < self.b.len() && !matches!(self.b[self.i], b'"' | b'\\' | 0..=0x1f) {
                self.i += 1;
            }
            out.extend_from_slice(&self.b[st..self.i]);
            match self.b.get(self.i) {
                None => return self.err("Unterminated string"),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(String::from_utf8_lossy(&out).into_owned());
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = self.b.get(self.i).copied();
                    self.i += 1;
                    let ch = match c {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            let mut u = self.hex4()?;
                            if (0xD800..0xDC00).contains(&u) && self.b[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                u = 0x10000 + ((u - 0xD800) << 10) + lo.wrapping_sub(0xDC00);
                            }
                            char::from_u32(u).unwrap_or('\u{fffd}')
                        }
                        _ => {
                            self.i -= 1;
                            return self.err("Invalid escape");
                        }
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                Some(_) => return self.err("Control character in a string"),
            }
        }
    }
}

fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Write `v`; `indent` is the spaces per level (None: minified).
pub fn write(v: &Json, indent: Option<usize>, out: &mut String) {
    fn go(v: &Json, ind: Option<usize>, lvl: usize, out: &mut String) {
        let nl = |out: &mut String, l: usize| {
            if let Some(n) = ind {
                out.push('\n');
                out.extend(std::iter::repeat_n(' ', n * l));
            }
        };
        match v {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => out.push_str(n),
            Json::Str(s) => escape(s, out),
            Json::Arr(a) if a.is_empty() => out.push_str("[]"),
            Json::Obj(m) if m.is_empty() => out.push_str("{}"),
            Json::Arr(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    nl(out, lvl + 1);
                    go(x, ind, lvl + 1, out);
                }
                nl(out, lvl);
                out.push(']');
            }
            Json::Obj(m) => {
                out.push('{');
                for (i, (k, x)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    nl(out, lvl + 1);
                    escape(k, out);
                    out.push_str(if ind.is_some() { ": " } else { ":" });
                    go(x, ind, lvl + 1, out);
                }
                nl(out, lvl);
                out.push('}');
            }
        }
    }
    go(v, indent, 0, out);
}

pub fn to_string(v: &Json, indent: Option<usize>) -> String {
    let mut s = String::new();
    write(v, indent, &mut s);
    s
}

/// Every object's keys in order (a stable sort, so a repeated key keeps its order).
pub fn sort_keys(v: &mut Json) {
    match v {
        Json::Arr(a) => a.iter_mut().for_each(sort_keys),
        Json::Obj(m) => {
            m.sort_by(|a, b| a.0.cmp(&b.0));
            m.iter_mut().for_each(|(_, x)| sort_keys(x));
        }
        _ => {}
    }
}

/// The page's four actions.
pub fn format(input: &str) -> Result<String, String> {
    parse(input).map(|v| to_string(&v, Some(2))).map_err(|e| e.to_string())
}

pub fn minify(input: &str) -> Result<String, String> {
    parse(input).map(|v| to_string(&v, None)).map_err(|e| e.to_string())
}

pub fn sorted(input: &str) -> Result<String, String> {
    parse(input).map(|mut v| {
        sort_keys(&mut v);
        to_string(&v, Some(2))
    }).map_err(|e| e.to_string())
}

pub fn validate(input: &str) -> Result<String, String> {
    match parse(input) {
        Ok(v) => {
            let kind = match v {
                Json::Arr(ref a) => format!("an array of {}", a.len()),
                Json::Obj(ref m) => format!("an object with {} keys", m.len()),
                _ => "a single value".into(),
            };
            Ok(format!("Valid JSON: {kind}, {} characters.", input.chars().count()))
        }
        Err(e) => Err(format!("Invalid JSON: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_minifies_and_sorts() {
        let src = r#"{"b":[1,2.50,{"z":null,"a":true}],"a":"x\ny"}"#;
        assert_eq!(minify(&format(src).unwrap()).unwrap(), src.replace("2.50", "2.50"));
        assert!(format(src).unwrap().contains("\n    1,\n    2.50,"));
        let s = sorted(src).unwrap();
        assert!(s.find("\"a\"").unwrap() < s.find("\"b\"").unwrap());
        assert!(s.contains("\"a\": true") && s.find("\"a\": true").unwrap() < s.find("\"z\"").unwrap());
        assert_eq!(minify("{ }").unwrap(), "{}");
    }

    #[test]
    fn errors_name_line_and_column() {
        let e = parse("{\n  \"a\": 1,\n  \"b\" 2\n}").unwrap_err();
        assert_eq!((e.line, e.col), (3, 7), "{e}");
        assert!(validate("[1,]").unwrap_err().contains("line 1, column 4"));
        assert!(parse("{\"a\":1} x").unwrap_err().msg.contains("after"));
        assert!(parse("\"abc").unwrap_err().msg.contains("Unterminated"));
        assert!(validate("{\"a\":[1,2]}").unwrap().starts_with("Valid JSON: an object with 1 keys"));
        let deep = "[".repeat(1000);
        assert!(parse(&deep).unwrap_err().msg.contains("deeply"));
    }

    #[test]
    fn escapes_and_unicode_survive() {
        let v = parse(r#""aé😀\t\"""#).unwrap();
        assert_eq!(v, Json::Str("a\u{e9}\u{1f600}\t\"".into()));
        assert_eq!(to_string(&v, None), "\"a\u{e9}\u{1f600}\\t\\\"\"");
        assert_eq!(minify("-0.5e+10").unwrap(), "-0.5e+10");
        assert!(parse("01x").is_err());
    }
}
