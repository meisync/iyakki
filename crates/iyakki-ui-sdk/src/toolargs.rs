//! The arguments of an app tool call, read without a dependency: the core hands an app the arguments as JSON text, an object whose
//! values are mostly strings, numbers and booleans. [`Args::parse`] reads one object (nested objects and arrays are kept as raw text),
//! with the strings unescaped. It refuses what is not an object, text that goes on after the object, and anything nested deeper than
//! 16 levels, with a sentence the app can give the model back.

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
    /// A nested object or array, as it was written.
    Raw(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args {
    pairs: Vec<(String, Val)>,
}

impl Args {
    pub fn parse(text: &str) -> Result<Args, String> {
        let mut p = P { b: text.as_bytes(), i: 0 };
        p.ws();
        p.expect(b'{')?;
        let mut pairs = Vec::new();
        p.ws();
        if p.peek() == Some(b'}') {
            p.i += 1;
        } else {
            loop {
                p.ws();
                let k = p.string()?;
                p.ws();
                p.expect(b':')?;
                p.ws();
                let v = p.value()?;
                pairs.push((k, v));
                p.ws();
                match p.next() {
                    Some(b',') => continue,
                    Some(b'}') => break,
                    _ => return Err("the arguments are not a JSON object".into()),
                }
            }
        }
        p.ws();
        if p.i != p.b.len() {
            return Err("the arguments have text after the object".into());
        }
        Ok(Args { pairs })
    }

    fn get(&self, k: &str) -> Option<&Val> {
        self.pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }

    pub fn str(&self, k: &str) -> Option<&str> {
        match self.get(k) {
            Some(Val::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// A whole number that is not negative (a JSON number such as `3` or `3.0`).
    pub fn uint(&self, k: &str) -> Option<u64> {
        match self.get(k) {
            Some(Val::Num(n)) if *n >= 0.0 && n.fract() == 0.0 && *n < 9.0e15 => Some(*n as u64),
            _ => None,
        }
    }

    pub fn bool(&self, k: &str) -> Option<bool> {
        match self.get(k) {
            Some(Val::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    pub fn has(&self, k: &str) -> bool {
        self.get(k).is_some()
    }
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn next(&mut self) -> Option<u8> {
        let c = self.peek();
        self.i += 1;
        c
    }
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.next() == Some(c) { Ok(()) } else { Err("the arguments are not a JSON object".into()) }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let s = self.b.get(self.i..self.i + 4).ok_or("a \\u escape is cut short")?;
        let t = std::str::from_utf8(s).map_err(|_| "a \\u escape is not hex")?;
        self.i += 4;
        u32::from_str_radix(t, 16).map_err(|_| "a \\u escape is not hex".to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"').map_err(|_| "a name or text is not in quotes".to_string())?;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = self.next().ok_or("a string is not closed")?;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self.next().ok_or("a string is not closed")?;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                                    return Err("a surrogate pair is cut short".into());
                                }
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err("a surrogate pair is wrong".into());
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            char::from_u32(cp).ok_or("a \\u escape is not a character")?
                        }
                        _ => return Err("a string has an unknown escape".into()),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                c if c < 0x20 => return Err("a string has a control character".into()),
                c => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| "a string is not text".to_string())
    }

    fn value(&mut self) -> Result<Val, String> {
        match self.peek() {
            Some(b'"') => self.string().map(Val::Str),
            Some(b'{') | Some(b'[') => {
                let start = self.i;
                self.skip_nested()?;
                Ok(Val::Raw(String::from_utf8_lossy(&self.b[start..self.i]).into_owned()))
            }
            Some(b't') if self.b[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(Val::Bool(true))
            }
            Some(b'f') if self.b[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(Val::Bool(false))
            }
            Some(b'n') if self.b[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(Val::Null)
            }
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                while matches!(self.peek(), Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')) {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.b[start..self.i]).unwrap_or("");
                t.parse::<f64>().map(Val::Num).map_err(|_| "a number is not readable".to_string())
            }
            _ => Err("a value is not readable".into()),
        }
    }

    fn skip_nested(&mut self) -> Result<(), String> {
        let mut depth = 0usize;
        loop {
            let c = self.next().ok_or("an object or list is not closed")?;
            match c {
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 16 {
                        return Err("the arguments are nested too deeply".into());
                    }
                }
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                b'"' => {
                    self.i -= 1;
                    self.string()?;
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_flat_objects_with_escapes_numbers_and_booleans() {
        let a = Args::parse(" {\"text\": \"milk \\\"2%\\\"\\n\\u00e9\\ud83d\\ude00\", \"n\": 3, \"f\": 2.5, \"on\": true, \"x\": null, \"o\": {\"a\": [1, \"}\"]}} ").unwrap();
        assert_eq!(a.str("text"), Some("milk \"2%\"\n\u{e9}\u{1f600}"));
        assert_eq!((a.uint("n"), a.uint("f"), a.bool("on")), (Some(3), None, Some(true)));
        assert!(a.has("x") && a.has("o") && !a.has("missing"));
        assert_eq!(a.str("n"), None);
        assert!(Args::parse("{}").is_ok());
    }

    #[test]
    fn refuses_what_is_not_an_object_with_a_sentence() {
        for bad in ["", "[1]", "{\"a\":}", "{\"a\":1} x", "{\"a\":\"x}", "{\"a\":\"\\q\"}", "{a:1}", "{\"a\":1,}", "{\"a\":\"\\ud800\"}"] {
            assert!(Args::parse(bad).is_err(), "{bad}");
        }
        let deep = format!("{{\"a\":{}{}}}", "[".repeat(40), "]".repeat(40));
        assert!(Args::parse(&deep).unwrap_err().contains("nested"));
    }
}
