//! A small JSON reader and writer for the plan file (`--plan-file`). In-house on purpose: the installer needs objects, arrays,
//! strings, whole numbers and booleans, nothing else. Numbers are unsigned or signed 64-bit integers (sizes and sector numbers
//! stay exact; there are no floats in a plan).

use std::fmt::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(i128),
    Str(String),
    Arr(Vec<Json>),
    /// Keys keep their order.
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj(fields: Vec<(&str, Json)>) -> Json {
        Json::Obj(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }
    pub fn num(n: u64) -> Json {
        Json::Num(n as i128)
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(f) => f.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Num(n) => u64::try_from(*n).ok(),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// Indented text, two spaces per level.
    pub fn pretty(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, 0);
        s.push('\n');
        s
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = |out: &mut String, d: usize| out.push_str(&"  ".repeat(d));
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                let _ = write!(out, "{n}");
            }
            Json::Str(s) => quote(s, out),
            Json::Arr(a) if a.is_empty() => out.push_str("[]"),
            Json::Arr(a) => {
                out.push_str("[\n");
                for (i, v) in a.iter().enumerate() {
                    pad(out, depth + 1);
                    v.write(out, depth + 1);
                    out.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
                }
                pad(out, depth);
                out.push(']');
            }
            Json::Obj(f) if f.is_empty() => out.push_str("{}"),
            Json::Obj(f) => {
                out.push_str("{\n");
                for (i, (k, v)) in f.iter().enumerate() {
                    pad(out, depth + 1);
                    quote(k, out);
                    out.push_str(": ");
                    v.write(out, depth + 1);
                    out.push_str(if i + 1 < f.len() { ",\n" } else { "\n" });
                }
                pad(out, depth);
                out.push('}');
            }
        }
    }

    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = P { b: text.as_bytes(), i: 0 };
        let v = p.value(0)?;
        p.ws();
        if p.i != p.b.len() {
            return Err(format!("unexpected text after the JSON value at byte {}", p.i));
        }
        Ok(v)
    }
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.ws();
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", c as char, self.i))
        }
    }
    fn lit(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(format!("unexpected text at byte {}", self.i))
        }
    }
    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > 32 {
            return Err("JSON nested too deeply".into());
        }
        self.ws();
        match self.b.get(self.i) {
            None => Err("unexpected end of the JSON text".into()),
            Some(b'n') => self.lit("null", Json::Null),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(a));
                        }
                        _ => return Err(format!("expected ',' or ']' at byte {}", self.i)),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut f = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(f));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.eat(b':')?;
                    f.push((k, self.value(depth + 1)?));
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(f));
                        }
                        _ => return Err(format!("expected ',' or '}}' at byte {}", self.i)),
                    }
                }
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                    self.i += 1;
                }
                if matches!(self.b.get(self.i), Some(b'.' | b'e' | b'E')) {
                    return Err("plan files hold whole numbers only".into());
                }
                std::str::from_utf8(&self.b[start..self.i]).ok().and_then(|s| s.parse::<i128>().ok()).map(Json::Num).ok_or_else(|| format!("bad number at byte {start}"))
            }
            Some(_) => Err(format!("unexpected character at byte {}", self.i)),
        }
    }
    fn string(&mut self) -> Result<String, String> {
        if self.b.get(self.i) != Some(&b'"') {
            return Err(format!("expected a string at byte {}", self.i));
        }
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = *self.b.get(self.i).ok_or("the JSON string is not closed")?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).map_err(|_| "the JSON string is not UTF-8".to_string()),
                b'\\' => {
                    let e = *self.b.get(self.i).ok_or("bad escape")?;
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = self.b.get(self.i..self.i + 4).and_then(|h| std::str::from_utf8(h).ok()).ok_or("bad \\u escape")?;
                            let mut cp = u32::from_str_radix(hex, 16).map_err(|_| "bad \\u escape")?;
                            self.i += 4;
                            if (0xD800..0xDC00).contains(&cp) && self.b.get(self.i..self.i + 2) == Some(b"\\u") {
                                let lo = self.b.get(self.i + 2..self.i + 6).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u32::from_str_radix(h, 16).ok()).ok_or("bad \\u escape")?;
                                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                                self.i += 6;
                            }
                            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err("bad escape in a JSON string".into()),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let v = Json::obj(vec![
            ("a", Json::num(u64::MAX)),
            ("neg", Json::Num(-5)),
            ("s", Json::str("quote \" back \\ nl \n tab \t é \u{1}")),
            ("arr", Json::Arr(vec![Json::Bool(true), Json::Null, Json::Arr(vec![]), Json::obj(vec![])])),
        ]);
        let text = v.pretty();
        assert_eq!(Json::parse(&text).unwrap(), v);
        assert_eq!(Json::parse(&text).unwrap().get("a").unwrap().as_u64(), Some(u64::MAX));
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        for bad in ["", "{", "[1,", "{\"a\" 1}", "tru", "\"abc", "1.5", "{\"a\":1} x", "[\"\\q\"]", "-"] {
            assert!(Json::parse(bad).is_err(), "{bad:?}");
        }
        let deep = "[".repeat(100) + &"]".repeat(100);
        assert!(Json::parse(&deep).unwrap_err().contains("deeply"));
        assert_eq!(Json::parse("\"\\ud83d\\ude00\"").unwrap(), Json::str("😀"));
    }
}
