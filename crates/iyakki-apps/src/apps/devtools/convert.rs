//! Converters: JSON <-> YAML (a simple subset), CSV <-> JSON, number bases and string case.

use super::json::{self, Json};

// ---- JSON -> YAML -------------------------------------------------------------------------------------------------

fn plain_ok(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    !s.is_empty()
        && s == s.trim()
        && !matches!(l.as_str(), "null" | "true" | "false" | "yes" | "no" | "on" | "off" | "~")
        && s.parse::<f64>().is_err()
        && !s.starts_with(['-', '?', ':', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`', '[', ']', '{', '}', ','])
        && !s.contains(": ")
        && !s.contains(" #")
        && !s.ends_with(':')
        && !s.contains(['\n', '\t'])
}

fn scalar(v: &Json) -> String {
    match v {
        Json::Str(s) if plain_ok(s) => s.clone(),
        Json::Str(s) => json::to_string(&Json::Str(s.clone()), None),
        Json::Arr(_) => "[]".into(),
        Json::Obj(_) => "{}".into(),
        other => json::to_string(other, None),
    }
}

fn is_container(v: &Json) -> bool {
    matches!(v, Json::Arr(a) if !a.is_empty()) || matches!(v, Json::Obj(m) if !m.is_empty())
}

fn key(k: &str) -> String {
    if plain_ok(k) { k.to_string() } else { json::to_string(&Json::Str(k.into()), None) }
}

fn emit(v: &Json, ind: usize, out: &mut String) {
    let pad = " ".repeat(ind);
    match v {
        Json::Obj(m) if !m.is_empty() => {
            for (k, x) in m {
                if is_container(x) {
                    out.push_str(&format!("{pad}{}:\n", key(k)));
                    emit(x, if matches!(x, Json::Arr(_)) { ind } else { ind + 2 }, out);
                } else {
                    out.push_str(&format!("{pad}{}: {}\n", key(k), scalar(x)));
                }
            }
        }
        Json::Arr(a) if !a.is_empty() => {
            for x in a {
                if is_container(x) {
                    let mut inner = String::new();
                    emit(x, ind + 2, &mut inner);
                    // the first line of a nested block shares the dash
                    out.push_str(&format!("{pad}- {}", inner.trim_start_matches(' ')));
                } else {
                    out.push_str(&format!("{pad}- {}\n", scalar(x)));
                }
            }
        }
        other => out.push_str(&format!("{pad}{}\n", scalar(other))),
    }
}

pub fn json_to_yaml(input: &str) -> Result<String, String> {
    let v = json::parse(input).map_err(|e| format!("Invalid JSON: {e}"))?;
    let mut s = String::new();
    emit(&v, 0, &mut s);
    Ok(s)
}

// ---- YAML -> JSON -------------------------------------------------------------------------------------------------

fn yscalar(t: &str) -> Result<Json, String> {
    let t = t.trim();
    Ok(match t {
        "" | "null" | "~" | "Null" | "NULL" => Json::Null,
        "true" | "True" | "TRUE" => Json::Bool(true),
        "false" | "False" | "FALSE" => Json::Bool(false),
        "[]" => Json::Arr(vec![]),
        "{}" => Json::Obj(vec![]),
        _ if t.starts_with('"') => json::parse(t).map_err(|e| format!("Bad quoted text {t}: {e}"))?,
        _ if t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2 => Json::Str(t[1..t.len() - 1].replace("''", "'")),
        _ if t.starts_with(['[', '{']) => json::parse(t).map_err(|_| "Only empty [] and {} (or valid JSON on one line) are understood inline".to_string())?,
        _ if t.parse::<f64>().is_ok() && !t.starts_with('+') && t.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '.' | 'e' | 'E' | '+')) => Json::Num(t.to_string()),
        _ => Json::Str(t.to_string()),
    })
}

/// Remove a trailing ` # comment` that is not inside quotes.
fn strip_comment(l: &str) -> &str {
    let mut q: Option<char> = None;
    let mut prev = ' ';
    for (i, c) in l.char_indices() {
        match q {
            Some(x) if c == x => q = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => q = Some(c),
            None if c == '#' && prev.is_whitespace() => return &l[..i],
            None => {}
        }
        prev = c;
    }
    l
}

struct Y {
    lines: Vec<(usize, usize, String)>,
    i: usize,
}

/// Split `key: value` at the first colon that ends a key.
fn split_key(t: &str) -> Option<(String, &str)> {
    if t.starts_with('"') || t.starts_with('\'') {
        let q = t.chars().next()?;
        let end = t[1..].find(q)? + 1;
        let rest = t[end + 1..].strip_prefix(':')?;
        return Some((t[1..end].to_string(), rest.trim()));
    }
    let p = t.find(": ").or_else(|| t.ends_with(':').then(|| t.len() - 1))?;
    Some((t[..p].trim().to_string(), t[p + 1..].trim()))
}

impl Y {
    fn block(&mut self, ind: usize) -> Result<Json, String> {
        let Some((_, _, first)) = self.lines.get(self.i) else { return Ok(Json::Null) };
        if first.starts_with("- ") || first == "-" {
            let mut a = Vec::new();
            while let Some((n, ln, t)) = self.lines.get(self.i).cloned() {
                if n != ind || !(t.starts_with("- ") || t == "-") {
                    if n > ind {
                        return Err(format!("Line {ln}: unexpected indent"));
                    }
                    break;
                }
                let rest = t[1..].trim_start().to_string();
                if rest.is_empty() {
                    self.i += 1;
                    a.push(if self.lines.get(self.i).is_some_and(|l| l.0 > ind) { self.block(self.lines[self.i].0)? } else { Json::Null });
                } else if split_key(&rest).is_some() {
                    // "- key: v" starts a mapping whose later keys are indented under the dash
                    let off = t.len() - rest.len();
                    self.lines[self.i] = (ind + off, ln, rest);
                    a.push(self.block(ind + off)?);
                } else {
                    self.i += 1;
                    a.push(yscalar(&rest)?);
                }
            }
            return Ok(Json::Arr(a));
        }
        let mut m = Vec::new();
        while let Some((n, ln, t)) = self.lines.get(self.i).cloned() {
            if n < ind {
                break;
            }
            if n > ind {
                return Err(format!("Line {ln}: unexpected indent"));
            }
            if t.starts_with("- ") {
                break;
            }
            let (k, v) = split_key(&t).ok_or_else(|| format!("Line {ln}: expected 'key: value'"))?;
            self.i += 1;
            let val = if v.is_empty() {
                match self.lines.get(self.i) {
                    Some((n2, _, t2)) if *n2 > ind || (*n2 == ind && t2.starts_with("- ")) => self.block(*n2)?,
                    _ => Json::Null,
                }
            } else if v == "|" || v == ">" {
                return Err(format!("Line {ln}: block text (| and >) is not supported"));
            } else {
                yscalar(v)?
            };
            m.push((k, val));
        }
        Ok(Json::Obj(m))
    }
}

pub fn yaml_to_json(input: &str) -> Result<String, String> {
    let mut lines = Vec::new();
    for (n, raw) in input.lines().enumerate() {
        let l = strip_comment(raw).trim_end();
        if l.trim().is_empty() || l.trim() == "---" {
            continue;
        }
        if l.contains('\t') && l.starts_with('\t') {
            return Err(format!("Line {}: YAML cannot indent with tabs", n + 1));
        }
        lines.push((l.len() - l.trim_start().len(), n + 1, l.trim_start().to_string()));
    }
    if lines.is_empty() {
        return Ok("null".into());
    }
    let ind = lines[0].0;
    let v = if lines.len() == 1 && split_key(&lines[0].2).is_none() && !lines[0].2.starts_with('-') { yscalar(&lines[0].2)? } else { Y { lines, i: 0 }.block(ind)? };
    Ok(json::to_string(&v, Some(2)))
}

// ---- CSV <-> JSON -------------------------------------------------------------------------------------------------

fn csv_rows(s: &str) -> Result<Vec<Vec<String>>, String> {
    let delim = if s.lines().next().is_some_and(|l| l.matches(';').count() > l.matches(',').count()) { ';' } else if s.lines().next().is_some_and(|l| l.contains('\t') && !l.contains(',')) { '\t' } else { ',' };
    let (mut rows, mut row, mut cell) = (Vec::new(), Vec::new(), String::new());
    let (mut q, mut any) = (false, false);
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        any = true;
        if q {
            if c == '"' {
                if it.peek() == Some(&'"') {
                    cell.push('"');
                    it.next();
                } else {
                    q = false;
                }
            } else {
                cell.push(c);
            }
        } else if c == '"' && cell.is_empty() {
            q = true;
        } else if c == delim {
            row.push(std::mem::take(&mut cell));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && it.peek() == Some(&'\n') {
                it.next();
            }
            row.push(std::mem::take(&mut cell));
            rows.push(std::mem::take(&mut row));
            any = false;
        } else {
            cell.push(c);
        }
    }
    if q {
        return Err("A quoted cell is never closed".into());
    }
    if any || !cell.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows.retain(|r| !(r.len() == 1 && r[0].is_empty()));
    Ok(rows)
}

pub fn csv_to_json(input: &str) -> Result<String, String> {
    let rows = csv_rows(input)?;
    let Some((head, body)) = rows.split_first() else { return Ok("[]".into()) };
    let items = body
        .iter()
        .map(|r| {
            Json::Obj(
                head.iter()
                    .enumerate()
                    .map(|(i, h)| {
                        let c = r.get(i).map_or("", |c| c.as_str());
                        let v = if !c.is_empty() && c.parse::<f64>().is_ok() && !c.starts_with(['+', '.']) && !(c.len() > 1 && c.starts_with('0') && !c.starts_with("0.")) && c.chars().all(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '.' | 'e' | 'E')) {
                            Json::Num(c.to_string())
                        } else {
                            match c {
                                "true" => Json::Bool(true),
                                "false" => Json::Bool(false),
                                _ => Json::Str(c.to_string()),
                            }
                        };
                        (h.clone(), v)
                    })
                    .collect(),
            )
        })
        .collect();
    Ok(json::to_string(&Json::Arr(items), Some(2)))
}

pub fn json_to_csv(input: &str) -> Result<String, String> {
    let v = json::parse(input).map_err(|e| format!("Invalid JSON: {e}"))?;
    let Json::Arr(items) = v else { return Err("CSV needs a JSON array of objects, like [{\"a\":1},{\"a\":2}].".into()) };
    let mut cols: Vec<String> = Vec::new();
    for it in &items {
        let Json::Obj(m) = it else { return Err("Every item of the array must be an object.".into()) };
        for (k, _) in m {
            if !cols.contains(k) {
                cols.push(k.clone());
            }
        }
    }
    let cell = |s: &str| if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    let mut out = cols.iter().map(|c| cell(c)).collect::<Vec<_>>().join(",") + "\n";
    for it in &items {
        let Json::Obj(m) = it else { continue };
        let row: Vec<String> = cols
            .iter()
            .map(|c| match m.iter().find(|(k, _)| k == c).map(|(_, v)| v) {
                None | Some(Json::Null) => String::new(),
                Some(Json::Str(s)) => cell(s),
                Some(other) => cell(&json::to_string(other, None)),
            })
            .collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    Ok(out)
}

// ---- numbers and case ---------------------------------------------------------------------------------------------

/// A whole number written in decimal, 0x hex, 0b binary or 0o octal (with _ or spaces allowed): shown in all four.
pub fn number_bases(input: &str) -> Result<String, String> {
    let t: String = input.trim().chars().filter(|c| *c != '_' && *c != ' ').collect();
    let (neg, t) = t.strip_prefix('-').map_or((false, t.as_str()), |r| (true, r));
    let (radix, digits) = match t.get(..2).map(|p| p.to_ascii_lowercase()).as_deref() {
        Some("0x") => (16, &t[2..]),
        Some("0b") => (2, &t[2..]),
        Some("0o") => (8, &t[2..]),
        _ => (10, t),
    };
    let n = u128::from_str_radix(digits, radix).map_err(|_| format!("Not a whole number in base {radix}. Use 255, 0xff, 0b1111_1111 or 0o377."))?;
    let s = if neg { "-" } else { "" };
    Ok(format!("Decimal  {s}{n}\nHex      {s}0x{n:x}\nBinary   {s}0b{n:b}\nOctal    {s}0o{n:o}\nBytes    {} byte(s) needed", (128 - n.leading_zeros()).div_ceil(8).max(1)))
}

fn words(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        // a lower-to-upper step (fooBar) or the end of an acronym (HTTPServer) starts a new word
        let split = i > 0 && c.is_uppercase() && (chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit() || (chars[i - 1].is_uppercase() && chars.get(i + 1).is_some_and(|n| n.is_lowercase())));
        if split && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Every case of each line of the input.
pub fn cases(input: &str) -> Result<String, String> {
    let cap = |w: &str| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
    };
    let mut out = String::new();
    for line in input.lines().filter(|l| !l.trim().is_empty()).take(50) {
        let w = words(line);
        let low: Vec<String> = w.iter().map(|x| x.to_lowercase()).collect();
        let pascal: String = w.iter().map(|x| cap(x)).collect();
        let mut camel = pascal.clone();
        if let Some(f) = low.first() {
            camel = f.clone() + &w[1..].iter().map(|x| cap(x)).collect::<String>();
        }
        out.push_str(&format!(
            "{line}\n  camelCase     {camel}\n  PascalCase    {pascal}\n  snake_case    {}\n  kebab-case    {}\n  CONSTANT_CASE {}\n  Title Case    {}\n  lower case    {}\n  UPPER CASE    {}\n",
            low.join("_"),
            low.join("-"),
            low.join("_").to_uppercase(),
            w.iter().map(|x| cap(x)).collect::<Vec<_>>().join(" "),
            low.join(" "),
            low.join(" ").to_uppercase()
        ));
    }
    if out.is_empty() {
        return Err("Type a word or phrase in the Input box.".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_to_yaml_and_back() {
        let j = r#"{"name":"x y","n":3,"ok":true,"none":null,"list":[1,"two",{"a":1,"b":[]}],"nest":{"k":"v: w","e":""},"s":"123"}"#;
        let y = json_to_yaml(j).unwrap();
        assert!(y.contains("name: x y\n") && y.contains("n: 3\n") && y.contains("list:\n- 1\n- two\n- a: 1\n  b: []\n"), "{y}");
        assert!(y.contains("k: \"v: w\"") && y.contains("e: \"\"") && y.contains("s: \"123\""), "{y}");
        let back = yaml_to_json(&y).unwrap();
        assert_eq!(json::parse(&back).unwrap(), json::parse(j).unwrap(), "{y}\n{back}");
    }

    #[test]
    fn yaml_subset_reads_comments_quotes_and_nested_sequences() {
        let y = "# top\nservers:\n  - name: a   # first\n    port: 80\n  - name: 'b c'\n    port: 8080\nflags: [1, 2]\nempty:\n";
        let j = json::parse(&yaml_to_json(y).unwrap()).unwrap();
        assert_eq!(json::to_string(&j, None), r#"{"servers":[{"name":"a","port":80},{"name":"b c","port":8080}],"flags":[1,2],"empty":null}"#);
        assert!(yaml_to_json("a: |\n  text").unwrap_err().contains("not supported"));
        assert!(yaml_to_json("a: 1\n   b: 2").unwrap_err().contains("indent"));
        assert_eq!(yaml_to_json("42").unwrap(), "42");
    }

    #[test]
    fn csv_and_json_both_ways_with_quotes() {
        let csv = "id,name,note\n1,\"Smith, A\",\"say \"\"hi\"\"\"\n2,Bo,\n";
        let j = csv_to_json(csv).unwrap();
        assert!(j.contains("\"id\": 1") && j.contains("\"Smith, A\"") && j.contains("say \\\"hi\\\""), "{j}");
        assert_eq!(json_to_csv(&j).unwrap(), csv);
        assert!(csv_to_json("a\n\"x").unwrap_err().contains("never closed"));
        assert!(json_to_csv("{}").is_err());
        assert_eq!(csv_to_json("a;b\n1;2").unwrap().matches('"').count(), 4);
    }

    #[test]
    fn number_bases_and_cases() {
        let n = number_bases("0xFF").unwrap();
        assert!(n.contains("Decimal  255") && n.contains("0b11111111") && n.contains("0o377") && n.contains("1 byte"), "{n}");
        assert!(number_bases("-1_000").unwrap().contains("-0x3e8"));
        assert!(number_bases("12z").is_err());
        let c = cases("HTTPServer fooBar_baz-qux").unwrap();
        for w in ["camelCase     httpServerFooBarBazQux", "snake_case    http_server_foo_bar_baz_qux", "kebab-case    http-server-foo-bar-baz-qux", "PascalCase    HttpServerFooBarBazQux", "Title Case    Http Server Foo Bar Baz Qux"] {
            assert!(c.contains(w), "{w}\n{c}");
        }
    }
}
