//! The small, safe expression language of conditions and `{{ templates }}`. No assignment, no loops, no calls except the fixed
//! list of pure functions below, no access to anything but the JSON context it is given. Length, depth and size are capped.
//! Values that a template inserts are never evaluated again (no injection through data).

use serde_json::{json, Number, Value};

pub const MAX_EXPR_LEN: usize = 2000;
pub const MAX_DEPTH: usize = 24;
pub const MAX_TEXT: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct ExprError(pub String);

impl std::fmt::Display for ExprError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(m: impl Into<String>) -> Result<T, ExprError> {
    Err(ExprError(m.into()))
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Id(String),
    Op(&'static str),
    End,
}

fn lex(s: &str) -> Result<Vec<Tok>, ExprError> {
    if s.len() > MAX_EXPR_LEN {
        return err(format!("an expression is at most {MAX_EXPR_LEN} characters"));
    }
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_ascii_digit() {
            let st = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.' && i + 1 < c.len() && c[i + 1].is_ascii_digit()) {
                i += 1;
            }
            let t: String = c[st..i].iter().collect();
            out.push(Tok::Num(t.parse().map_err(|_| ExprError(format!("`{t}` is not a number")))?));
        } else if ch == '"' || ch == '\'' {
            let q = ch;
            i += 1;
            let mut t = String::new();
            loop {
                match c.get(i) {
                    None => return err("a text is not closed"),
                    Some(&x) if x == q => {
                        i += 1;
                        break;
                    }
                    Some('\\') => {
                        match c.get(i + 1) {
                            Some('n') => t.push('\n'),
                            Some('t') => t.push('\t'),
                            Some(&x) => t.push(x),
                            None => return err("a text is not closed"),
                        }
                        i += 2;
                    }
                    Some(&x) => {
                        t.push(x);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Str(t));
        } else if ch.is_alphabetic() || ch == '_' {
            let st = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '-' && i + 1 < c.len() && c[i + 1].is_alphanumeric() && out.last() == Some(&Tok::Op("."))) {
                i += 1;
            }
            out.push(Tok::Id(c[st..i].iter().collect()));
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            let op = match two.as_str() {
                "==" => Some("=="),
                "!=" => Some("!="),
                "<=" => Some("<="),
                ">=" => Some(">="),
                "&&" => Some("&&"),
                "||" => Some("||"),
                _ => None,
            };
            if let Some(o) = op {
                out.push(Tok::Op(o));
                i += 2;
                continue;
            }
            let o = match ch {
                '<' => "<",
                '>' => ">",
                '+' => "+",
                '-' => "-",
                '*' => "*",
                '/' => "/",
                '%' => "%",
                '!' => "!",
                '(' => "(",
                ')' => ")",
                '[' => "[",
                ']' => "]",
                ',' => ",",
                '.' => ".",
                other => return err(format!("unexpected `{other}`")),
            };
            out.push(Tok::Op(o));
            i += 1;
        }
    }
    out.push(Tok::End);
    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ast {
    Lit(Value),
    Path(String, Vec<Seg>),
    Call(String, Vec<Ast>),
    List(Vec<Ast>),
    Unary(&'static str, Box<Ast>),
    Bin(&'static str, Box<Ast>, Box<Ast>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Key(String),
    Index(Box<Ast>),
}

struct Parser {
    t: Vec<Tok>,
    i: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.t[self.i]
    }
    fn next(&mut self) -> Tok {
        let t = self.t[self.i].clone();
        if self.i + 1 < self.t.len() {
            self.i += 1;
        }
        t
    }
    fn eat(&mut self, op: &str) -> bool {
        if matches!(self.peek(), Tok::Op(o) if *o == op) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn enter(&mut self) -> Result<(), ExprError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return err("the expression is nested too deeply");
        }
        Ok(())
    }

    fn or(&mut self) -> Result<Ast, ExprError> {
        self.enter()?;
        let mut l = self.and()?;
        while self.eat("||") {
            l = Ast::Bin("||", Box::new(l), Box::new(self.and()?));
        }
        self.depth -= 1;
        Ok(l)
    }
    fn and(&mut self) -> Result<Ast, ExprError> {
        let mut l = self.cmp()?;
        while self.eat("&&") {
            l = Ast::Bin("&&", Box::new(l), Box::new(self.cmp()?));
        }
        Ok(l)
    }
    fn cmp(&mut self) -> Result<Ast, ExprError> {
        let l = self.add()?;
        for op in ["==", "!=", "<=", ">=", "<", ">"] {
            if self.eat(op) {
                return Ok(Ast::Bin(op, Box::new(l), Box::new(self.add()?)));
            }
        }
        if matches!(self.peek(), Tok::Id(w) if w == "in") {
            self.i += 1;
            return Ok(Ast::Bin("in", Box::new(l), Box::new(self.add()?)));
        }
        Ok(l)
    }
    fn add(&mut self) -> Result<Ast, ExprError> {
        let mut l = self.mul()?;
        loop {
            let op = if self.eat("+") {
                "+"
            } else if self.eat("-") {
                "-"
            } else {
                break;
            };
            l = Ast::Bin(op, Box::new(l), Box::new(self.mul()?));
        }
        Ok(l)
    }
    fn mul(&mut self) -> Result<Ast, ExprError> {
        let mut l = self.unary()?;
        loop {
            let op = if self.eat("*") {
                "*"
            } else if self.eat("/") {
                "/"
            } else if self.eat("%") {
                "%"
            } else {
                break;
            };
            l = Ast::Bin(op, Box::new(l), Box::new(self.unary()?));
        }
        Ok(l)
    }
    fn unary(&mut self) -> Result<Ast, ExprError> {
        self.enter()?;
        let r = if self.eat("!") {
            Ast::Unary("!", Box::new(self.unary()?))
        } else if self.eat("-") {
            Ast::Unary("-", Box::new(self.unary()?))
        } else {
            self.primary()?
        };
        self.depth -= 1;
        Ok(r)
    }
    fn primary(&mut self) -> Result<Ast, ExprError> {
        match self.next() {
            Tok::Num(n) => Ok(Ast::Lit(num(n))),
            Tok::Str(s) => Ok(Ast::Lit(Value::String(s))),
            Tok::Op("(") => {
                let e = self.or()?;
                if !self.eat(")") {
                    return err("a `)` is missing");
                }
                Ok(e)
            }
            Tok::Op("[") => {
                let mut v = Vec::new();
                if !self.eat("]") {
                    loop {
                        v.push(self.or()?);
                        if self.eat("]") {
                            break;
                        }
                        if !self.eat(",") {
                            return err("a `,` or `]` is missing in the list");
                        }
                    }
                }
                Ok(Ast::List(v))
            }
            Tok::Id(w) => match w.as_str() {
                "true" => Ok(Ast::Lit(Value::Bool(true))),
                "false" => Ok(Ast::Lit(Value::Bool(false))),
                "null" => Ok(Ast::Lit(Value::Null)),
                _ if self.eat("(") => {
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.or()?);
                            if self.eat(")") {
                                break;
                            }
                            if !self.eat(",") {
                                return err("a `,` or `)` is missing in the call");
                            }
                        }
                    }
                    if !FUNCS.contains(&w.as_str()) {
                        return err(format!("`{w}` is not a function (allowed: {})", FUNCS.join(", ")));
                    }
                    Ok(Ast::Call(w, args))
                }
                _ => {
                    let mut segs = Vec::new();
                    loop {
                        if self.eat(".") {
                            match self.next() {
                                Tok::Id(k) => segs.push(Seg::Key(k)),
                                Tok::Num(n) => segs.push(Seg::Key(format!("{}", n as i64))),
                                _ => return err("a name is missing after `.`"),
                            }
                        } else if self.eat("[") {
                            let e = self.or()?;
                            if !self.eat("]") {
                                return err("a `]` is missing");
                            }
                            segs.push(Seg::Index(Box::new(e)));
                        } else {
                            break;
                        }
                    }
                    Ok(Ast::Path(w, segs))
                }
            },
            Tok::End => err("the expression ends too soon"),
            Tok::Op(o) => err(format!("unexpected `{o}`")),
        }
    }
}

pub const FUNCS: [&str; 26] = ["len", "contains", "starts_with", "ends_with", "lower", "upper", "trim", "int", "float", "str", "default", "exists", "is_null", "join", "first", "last", "keys", "min", "max", "abs", "round", "split", "replace", "json", "not_empty", "bool"];

fn num(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9e15 {
        json!(n as i64)
    } else {
        Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

pub fn parse(src: &str) -> Result<Ast, ExprError> {
    let mut p = Parser { t: lex(src)?, i: 0, depth: 0 };
    let a = p.or()?;
    if *p.peek() != Tok::End {
        return err(format!("unexpected {:?} after the expression", p.peek()));
    }
    Ok(a)
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(*b as u8 as f64),
        _ => None,
    }
}

pub fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A path through a JSON value: `a.b[0].c` or `a.0.c`. Missing is null.
pub fn get_path<'a>(v: &'a Value, path: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    let mut cur = v;
    for part in path.split('.').filter(|p| !p.is_empty()) {
        let (name, rest) = part.split_once('[').map_or((part, ""), |(a, b)| (a, b));
        if !name.is_empty() {
            cur = match cur {
                Value::Object(o) => o.get(name).unwrap_or(&NULL),
                Value::Array(a) => name.parse::<usize>().ok().and_then(|i| a.get(i)).unwrap_or(&NULL),
                _ => &NULL,
            };
        }
        for idx in rest.split('[').filter(|x| !x.is_empty()) {
            let n = idx.trim_end_matches(']');
            cur = match cur {
                Value::Array(a) => n.parse::<usize>().ok().and_then(|i| a.get(i)).unwrap_or(&NULL),
                Value::Object(o) => o.get(n).unwrap_or(&NULL),
                _ => &NULL,
            };
        }
    }
    cur
}

fn eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

fn order(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64()?.partial_cmp(&y.as_f64()?),
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

pub fn eval(ast: &Ast, ctx: &Value) -> Result<Value, ExprError> {
    match ast {
        Ast::Lit(v) => Ok(v.clone()),
        Ast::List(items) => Ok(Value::Array(items.iter().map(|a| eval(a, ctx)).collect::<Result<_, _>>()?)),
        Ast::Path(root, segs) => {
            let mut cur = match ctx {
                Value::Object(o) => o.get(root).cloned().unwrap_or(Value::Null),
                _ => Value::Null,
            };
            for s in segs {
                let key = match s {
                    Seg::Key(k) => Value::String(k.clone()),
                    Seg::Index(e) => eval(e, ctx)?,
                };
                cur = match (&cur, &key) {
                    (Value::Object(o), Value::String(k)) => o.get(k).cloned().unwrap_or(Value::Null),
                    (Value::Array(a), Value::String(k)) => k.parse::<usize>().ok().and_then(|i| a.get(i).cloned()).unwrap_or(Value::Null),
                    (Value::Array(a), Value::Number(n)) => n.as_u64().and_then(|i| a.get(i as usize).cloned()).unwrap_or(Value::Null),
                    (Value::Object(o), Value::Number(n)) => o.get(&n.to_string()).cloned().unwrap_or(Value::Null),
                    _ => Value::Null,
                };
            }
            Ok(cur)
        }
        Ast::Unary(op, e) => {
            let v = eval(e, ctx)?;
            match *op {
                "!" => Ok(Value::Bool(!truthy(&v))),
                _ => match as_f64(&v) {
                    Some(x) => Ok(num(-x)),
                    None => err("`-` needs a number"),
                },
            }
        }
        Ast::Bin(op, l, r) => {
            if *op == "&&" {
                let a = eval(l, ctx)?;
                return if truthy(&a) { eval(r, ctx) } else { Ok(a) };
            }
            if *op == "||" {
                let a = eval(l, ctx)?;
                return if truthy(&a) { Ok(a) } else { eval(r, ctx) };
            }
            let (a, b) = (eval(l, ctx)?, eval(r, ctx)?);
            match *op {
                "==" => Ok(Value::Bool(eq(&a, &b))),
                "!=" => Ok(Value::Bool(!eq(&a, &b))),
                "<" | "<=" | ">" | ">=" => {
                    let Some(o) = order(&a, &b) else { return Ok(Value::Bool(false)) };
                    use std::cmp::Ordering::*;
                    Ok(Value::Bool(match *op {
                        "<" => o == Less,
                        "<=" => o != Greater,
                        ">" => o == Greater,
                        _ => o != Less,
                    }))
                }
                "in" => Ok(Value::Bool(match &b {
                    Value::Array(items) => items.iter().any(|x| eq(x, &a)),
                    Value::String(s) => s.contains(&text(&a)),
                    Value::Object(o) => o.contains_key(&text(&a)),
                    _ => false,
                })),
                "+" => match (&a, &b) {
                    (Value::Array(x), Value::Array(y)) => Ok(Value::Array(x.iter().chain(y.iter()).cloned().collect())),
                    (Value::String(_), _) | (_, Value::String(_)) if as_f64(&a).is_none() || as_f64(&b).is_none() || matches!(a, Value::String(_)) && matches!(b, Value::String(_)) => {
                        let s = format!("{}{}", text(&a), text(&b));
                        if s.len() > MAX_TEXT {
                            return err("the text is too long");
                        }
                        Ok(Value::String(s))
                    }
                    _ => match (as_f64(&a), as_f64(&b)) {
                        (Some(x), Some(y)) => Ok(num(x + y)),
                        _ => err("`+` needs two numbers or some text"),
                    },
                },
                "-" | "*" | "/" | "%" => match (as_f64(&a), as_f64(&b)) {
                    (Some(x), Some(y)) => {
                        if matches!(*op, "/" | "%") && y == 0.0 {
                            return err("division by zero");
                        }
                        Ok(num(match *op {
                            "-" => x - y,
                            "*" => x * y,
                            "/" => x / y,
                            _ => x % y,
                        }))
                    }
                    _ => err(format!("`{op}` needs two numbers")),
                },
                _ => err("unknown operator"),
            }
        }
        Ast::Call(name, args) => {
            let a: Vec<Value> = args.iter().map(|x| eval(x, ctx)).collect::<Result<_, _>>()?;
            call(name, &a)
        }
    }
}

fn arg<'a>(a: &'a [Value], i: usize, f: &str) -> Result<&'a Value, ExprError> {
    a.get(i).ok_or_else(|| ExprError(format!("`{f}` needs more arguments")))
}

fn call(f: &str, a: &[Value]) -> Result<Value, ExprError> {
    Ok(match f {
        "len" => json!(match arg(a, 0, f)? {
            Value::String(s) => s.chars().count(),
            Value::Array(x) => x.len(),
            Value::Object(o) => o.len(),
            _ => 0,
        }),
        "contains" => Value::Bool(match arg(a, 0, f)? {
            Value::String(s) => s.contains(&text(arg(a, 1, f)?)),
            Value::Array(x) => x.iter().any(|v| eq(v, arg(a, 1, f).unwrap_or(&Value::Null))),
            Value::Object(o) => o.contains_key(&text(arg(a, 1, f)?)),
            _ => false,
        }),
        "starts_with" => Value::Bool(text(arg(a, 0, f)?).starts_with(&text(arg(a, 1, f)?))),
        "ends_with" => Value::Bool(text(arg(a, 0, f)?).ends_with(&text(arg(a, 1, f)?))),
        "lower" => Value::String(text(arg(a, 0, f)?).to_lowercase()),
        "upper" => Value::String(text(arg(a, 0, f)?).to_uppercase()),
        "trim" => Value::String(text(arg(a, 0, f)?).trim().to_string()),
        "int" => as_f64(arg(a, 0, f)?).map_or(Value::Null, |x| json!(x.trunc() as i64)),
        "float" => as_f64(arg(a, 0, f)?).map_or(Value::Null, num),
        "str" => Value::String(text(arg(a, 0, f)?)),
        "bool" => Value::Bool(truthy(arg(a, 0, f)?)),
        "json" => Value::String(arg(a, 0, f)?.to_string()),
        "default" => {
            let v = arg(a, 0, f)?;
            if v.is_null() || matches!(v, Value::String(s) if s.is_empty()) {
                arg(a, 1, f)?.clone()
            } else {
                v.clone()
            }
        }
        "exists" | "not_empty" => Value::Bool(!arg(a, 0, f)?.is_null() && (f == "exists" || truthy(&a[0]))),
        "is_null" => Value::Bool(arg(a, 0, f)?.is_null()),
        "join" => match arg(a, 0, f)? {
            Value::Array(x) => Value::String(x.iter().map(text).collect::<Vec<_>>().join(&a.get(1).map(text).unwrap_or_default())),
            other => Value::String(text(other)),
        },
        "first" => match arg(a, 0, f)? {
            Value::Array(x) => x.first().cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        },
        "last" => match arg(a, 0, f)? {
            Value::Array(x) => x.last().cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        },
        "keys" => match arg(a, 0, f)? {
            Value::Object(o) => Value::Array(o.keys().map(|k| Value::String(k.clone())).collect()),
            _ => Value::Array(vec![]),
        },
        "min" | "max" => {
            let items: Vec<f64> = if let [Value::Array(x)] = a { x.iter().filter_map(as_f64).collect() } else { a.iter().filter_map(as_f64).collect() };
            let r = if f == "min" { items.iter().cloned().fold(f64::INFINITY, f64::min) } else { items.iter().cloned().fold(f64::NEG_INFINITY, f64::max) };
            if items.is_empty() {
                Value::Null
            } else {
                num(r)
            }
        }
        "abs" => as_f64(arg(a, 0, f)?).map_or(Value::Null, |x| num(x.abs())),
        "round" => {
            let d = a.get(1).and_then(as_f64).unwrap_or(0.0).clamp(0.0, 9.0) as i32;
            as_f64(arg(a, 0, f)?).map_or(Value::Null, |x| num((x * 10f64.powi(d)).round() / 10f64.powi(d)))
        }
        "split" => Value::Array(text(arg(a, 0, f)?).split(&text(arg(a, 1, f)?)).take(10_000).map(|s| Value::String(s.to_string())).collect()),
        "replace" => {
            let (s, from) = (text(arg(a, 0, f)?), text(arg(a, 1, f)?));
            if from.is_empty() {
                Value::String(s)
            } else {
                let r = s.replace(&from, &text(arg(a, 2, f)?));
                if r.len() > MAX_TEXT {
                    return err("the text is too long");
                }
                Value::String(r)
            }
        }
        _ => return err(format!("`{f}` is not a function")),
    })
}

/// Parse and evaluate one expression.
pub fn eval_str(src: &str, ctx: &Value) -> Result<Value, ExprError> {
    eval(&parse(src)?, ctx)
}

/// A condition: the expression's truth. An empty expression is true.
pub fn condition(src: &str, ctx: &Value) -> Result<bool, ExprError> {
    if src.trim().is_empty() {
        return Ok(true);
    }
    Ok(truthy(&eval_str(src, ctx)?))
}

// ------------------------------------------------------------------------------------------------ templates

/// Looks up `{{secret.NAME}}`. `None` means this field may not carry a secret.
pub type SecretFn<'a> = Option<&'a dyn Fn(&str) -> Result<String, String>>;

/// Fill in the `{{ ... }}` of one string. Exactly one template and nothing else keeps the value's type.
pub fn render(s: &str, ctx: &Value, secrets: SecretFn) -> Result<Value, ExprError> {
    if !s.contains("{{") {
        return Ok(Value::String(s.to_string()));
    }
    let parts = split_template(s)?;
    if let [Part::Expr(e)] = parts.as_slice() {
        return eval_part(e, ctx, secrets);
    }
    let mut out = String::new();
    for p in parts {
        match p {
            Part::Text(t) => out.push_str(&t),
            Part::Expr(e) => out.push_str(&text(&eval_part(&e, ctx, secrets)?)),
        }
        if out.len() > MAX_TEXT {
            return err("the text is too long");
        }
    }
    Ok(Value::String(out))
}

enum Part {
    Text(String),
    Expr(String),
}

fn split_template(s: &str) -> Result<Vec<Part>, ExprError> {
    let mut parts = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("{{") {
        if i > 0 {
            parts.push(Part::Text(rest[..i].to_string()));
        }
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else { return err("a `{{` is not closed") };
        parts.push(Part::Expr(after[..j].trim().to_string()));
        rest = &after[j + 2..];
        if parts.len() > 200 {
            return err("too many templates in one text");
        }
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest.to_string()));
    }
    Ok(parts)
}

fn eval_part(e: &str, ctx: &Value, secrets: SecretFn) -> Result<Value, ExprError> {
    if let Some(name) = e.strip_prefix("secret.") {
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')) {
            return err("a secret name is letters, digits, `_`, `-`, `.`");
        }
        return match secrets {
            Some(f) => f(name).map(Value::String).map_err(ExprError),
            None => err("a secret can only be used in a header, credential or environment field"),
        };
    }
    eval_str(e, ctx)
}

/// The templates a text uses, as expression sources (for validation and for finding what a node reads).
pub fn templates_in(s: &str) -> Vec<String> {
    split_template(s).map(|p| p.into_iter().filter_map(|x| if let Part::Expr(e) = x { Some(e) } else { None }).collect()).unwrap_or_default()
}

/// Fill in every string inside a JSON value (keys are left alone). Results are data: never rendered again.
pub fn resolve(v: &Value, ctx: &Value, secrets: SecretFn) -> Result<Value, ExprError> {
    Ok(match v {
        Value::String(s) => render(s, ctx, secrets)?,
        Value::Array(a) => Value::Array(a.iter().map(|x| resolve(x, ctx, secrets)).collect::<Result<_, _>>()?),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| Ok((k.clone(), resolve(x, ctx, secrets)?))).collect::<Result<_, ExprError>>()?),
        other => other.clone(),
    })
}

/// Names that a flow value reads: every `nodes.<id>` path root used by templates or expressions in the JSON.
pub fn node_refs(v: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                for e in templates_in(s) {
                    refs_in_expr(&e, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(o) => o.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, &mut out);
    out.sort();
    out.dedup();
    out
}

/// `nodes.a.output.x` references (the node id and the first output key) in an expression's source.
pub fn refs_in_expr(src: &str, out: &mut Vec<String>) {
    let mut rest = src;
    while let Some(i) = rest.find("nodes.") {
        let ok_start = i == 0 || !rest[..i].chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.');
        let after = &rest[i + 6..];
        let id: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
        if ok_start && !id.is_empty() {
            let tail = &after[id.len()..];
            let key = tail.strip_prefix(".output.").map(|t| t.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<String>());
            out.push(match key {
                Some(k) if !k.is_empty() => format!("{id}.{k}"),
                _ => id,
            });
        }
        rest = &rest[i + 6..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Value {
        json!({"inputs": {"n": 3, "name": "Ada", "tags": ["a", "b"]}, "nodes": {"a": {"output": {"x": 7, "ok": true, "list": [1, 2, 3], "msg": "hello"}, "status": "succeeded"}}})
    }

    fn ev(s: &str) -> Value {
        eval_str(s, &ctx()).unwrap()
    }

    #[test]
    fn arithmetic_logic_and_paths() {
        assert_eq!(ev("1 + 2 * 3"), json!(7));
        assert_eq!(ev("(1 + 2) * 3"), json!(9));
        assert_eq!(ev("nodes.a.output.x > 5 && inputs.n == 3"), json!(true));
        assert_eq!(ev("!nodes.a.output.ok || false"), json!(false));
        assert_eq!(ev("nodes.a.output.list[1]"), json!(2));
        assert_eq!(ev("nodes.a.output.list.2"), json!(3));
        assert_eq!(ev("nodes.nope.output.x"), Value::Null, "a missing path is null, not an error");
        assert_eq!(ev("'a' in inputs.tags"), json!(true));
        assert_eq!(ev("len(inputs.tags) + 1"), json!(3));
        assert_eq!(ev("\"x\" + 1"), json!("x1"));
        assert_eq!(ev("10 / 4"), json!(2.5));
        assert_eq!(ev("7 % 4"), json!(3));
        assert_eq!(ev("-inputs.n"), json!(-3));
        assert_eq!(ev("default(nodes.zz.output.q, 'dflt')"), json!("dflt"));
        assert_eq!(ev("upper(trim(' hi '))"), json!("HI"));
        assert_eq!(ev("max(inputs.n, 9, 4)"), json!(9));
        assert_eq!(ev("round(2.456, 1)"), json!(2.5));
        assert_eq!(ev("join(split('a,b,c', ','), '-')"), json!("a-b-c"));
        assert_eq!(ev("starts_with(nodes.a.output.msg, 'he') && contains(nodes.a.output.msg, 'ell')"), json!(true));
        assert_eq!(ev("nodes.a.status == 'succeeded'"), json!(true));
        assert_eq!(ev("1 < 2 && 'a' < 'b'"), json!(true));
        assert_eq!(ev("1 < 'a'"), json!(false));
    }

    #[test]
    fn errors_are_plain_and_nothing_dangerous_exists() {
        for bad in ["1 +", "(1", "foo(1)", "system('x')", "a = 1", "1 / 0", "$x", "[1,", "'open"] {
            assert!(eval_str(bad, &ctx()).is_err(), "{bad}");
        }
        assert!(parse(&"(".repeat(200)).unwrap_err().0.contains("deeply"));
        assert!(parse(&"1+".repeat(2000)).unwrap_err().0.contains("at most"));
        // no way to reach anything but the context
        assert_eq!(ev("env"), Value::Null);
        assert_eq!(ev("__proto__.constructor"), Value::Null);
    }

    #[test]
    fn templates_keep_types_and_never_re_evaluate_inserted_data() {
        let c = json!({"inputs": {"evil": "{{ 1 + 1 }}", "n": 5}, "nodes": {"a": {"output": {"x": [1, 2]}}}});
        assert_eq!(render("{{ inputs.n }}", &c, None).unwrap(), json!(5));
        assert_eq!(render("n={{inputs.n}}!", &c, None).unwrap(), json!("n=5!"));
        assert_eq!(render("{{ nodes.a.output.x }}", &c, None).unwrap(), json!([1, 2]));
        assert_eq!(render("{{ inputs.evil }}", &c, None).unwrap(), json!("{{ 1 + 1 }}"));
        assert_eq!(render("x {{ inputs.evil }} y", &c, None).unwrap(), json!("x {{ 1 + 1 }} y"));
        assert_eq!(render("plain", &c, None).unwrap(), json!("plain"));
        assert!(render("{{ 1 +", &c, None).is_err());
        assert_eq!(render("a{{inputs.missing}}b", &c, None).unwrap(), json!("ab"));
        let r = resolve(&json!({"u": "http://h/{{inputs.n}}", "l": ["{{inputs.n}}", 2], "k": {"{{inputs.n}}": 1}}), &c, None).unwrap();
        assert_eq!(r, json!({"u": "http://h/5", "l": [5, 2], "k": {"{{inputs.n}}": 1}}), "keys are not templated");
    }

    #[test]
    fn secrets_only_where_allowed() {
        let c = json!({});
        assert!(render("Bearer {{secret.tok}}", &c, None).unwrap_err().0.contains("header"));
        let f = |n: &str| if n == "tok" { Ok("S3CRET".to_string()) } else { Err(format!("no secret {n}")) };
        assert_eq!(render("Bearer {{secret.tok}}", &c, Some(&f)).unwrap(), json!("Bearer S3CRET"));
        assert!(render("{{secret.other}}", &c, Some(&f)).is_err());
        assert!(render("{{secret.a b}}", &c, Some(&f)).is_err());
    }

    #[test]
    fn what_a_value_reads() {
        let v = json!({"a": "{{ nodes.fetch.output.items }}", "b": ["x {{ nodes.fetch.output.count + nodes.other.status }}"], "c": "{{ inputs.n }}"});
        assert_eq!(node_refs(&v), vec!["fetch.count", "fetch.items", "other"]);
    }

    #[test]
    fn conditions() {
        assert!(condition("", &ctx()).unwrap());
        assert!(condition("nodes.a.output.ok", &ctx()).unwrap());
        assert!(!condition("nodes.a.output.nothing", &ctx()).unwrap());
        assert!(condition("[1] && 'x'", &ctx()).unwrap());
    }
}
