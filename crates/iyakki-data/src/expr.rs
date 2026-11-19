//! A small expression language for filters and derived columns, shared by both engines.
//!
//! ```text
//! price > 10 and contains(name, "tea")        text is "double" or 'single' quoted
//! `unit price` * qty                           columns with spaces or odd characters: `backticks` or [brackets]
//! city in ("Paris", "Rome") or not is_null(x)  also: x is null / x is not null
//! round(price * 1.2, 2)                        + - * / %, comparisons = == != <> < <= > >=, and / or / not
//! ```
//!
//! Functions: `contains starts_with ends_with lower upper trim len abs round floor ceil coalesce concat is_null`.
//! The parser makes an [`Expr`] (serializable); [`check`] types it against a schema before anything runs, so mistakes
//! come back as plain sentences naming the column or the function.

use serde::{Deserialize, Serialize};

use crate::model::{Column, DType, DataError, Result};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Lit {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Lit(Lit),
    Col(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    In(Box<Expr>, Vec<Expr>),
    IsNull(Box<Expr>, bool),
}

// ------------------------------------------------------------------ lexer

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(String),
    Str(String),
    Ident(String),
    Quoted(String),
    Op(&'static str),
}

fn lex(src: &str) -> Result<Vec<Tok>> {
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    let bad = |m: String| DataError::BadRequest(m);
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let s = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.' || c[i] == '_') {
                i += 1;
            }
            if i < c.len() && (c[i] == 'e' || c[i] == 'E') && c.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == '-' || *d == '+') {
                i += 2;
                while i < c.len() && c[i].is_ascii_digit() {
                    i += 1;
                }
            }
            out.push(Tok::Num(c[s..i].iter().filter(|x| **x != '_').collect()));
        } else if ch == '"' || ch == '\'' {
            let q = ch;
            i += 1;
            let mut s = String::new();
            loop {
                match c.get(i) {
                    None => return Err(bad(format!("the text starting with {q} is never closed"))),
                    Some(&x) if x == q => {
                        if c.get(i + 1) == Some(&q) {
                            s.push(q);
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    }
                    Some('\\') if c.get(i + 1).is_some_and(|n| *n == q || *n == '\\') => {
                        s.push(c[i + 1]);
                        i += 2;
                    }
                    Some(&x) => {
                        s.push(x);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Str(s));
        } else if ch == '`' || ch == '[' {
            let close = if ch == '`' { '`' } else { ']' };
            let s = i + 1;
            let Some(e) = c[s..].iter().position(|x| *x == close) else { return Err(bad(format!("the column name starting with {ch} is never closed"))) };
            out.push(Tok::Quoted(c[s..s + e].iter().collect()));
            i = s + e + 1;
        } else if ch.is_alphabetic() || ch == '_' {
            let s = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '.') {
                i += 1;
            }
            out.push(Tok::Ident(c[s..i].iter().collect()));
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            let op = match two.as_str() {
                "==" => Some("="),
                "!=" | "<>" => Some("!="),
                "<=" => Some("<="),
                ">=" => Some(">="),
                "&&" => Some("and"),
                "||" => Some("or"),
                _ => None,
            };
            if let Some(op) = op {
                out.push(Tok::Op(op));
                i += 2;
                continue;
            }
            let op = match ch {
                '+' => "+",
                '-' => "-",
                '*' => "*",
                '/' => "/",
                '%' => "%",
                '=' => "=",
                '<' => "<",
                '>' => ">",
                '(' => "(",
                ')' => ")",
                ',' => ",",
                '!' => "not",
                other => return Err(bad(format!("unexpected {other:?} in the expression"))),
            };
            out.push(Tok::Op(op));
            i += 1;
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ parser

struct P {
    t: Vec<Tok>,
    i: usize,
}

const FUNCS: &[(&str, usize, usize)] = &[
    ("contains", 2, 2),
    ("starts_with", 2, 2),
    ("ends_with", 2, 2),
    ("lower", 1, 1),
    ("upper", 1, 1),
    ("trim", 1, 1),
    ("len", 1, 1),
    ("abs", 1, 1),
    ("round", 1, 2),
    ("floor", 1, 1),
    ("ceil", 1, 1),
    ("coalesce", 2, 8),
    ("concat", 2, 8),
    ("is_null", 1, 1),
];

impl P {
    fn peek_kw(&self, kw: &str) -> bool {
        match self.t.get(self.i) {
            Some(Tok::Ident(s)) => s.eq_ignore_ascii_case(kw),
            Some(Tok::Op(o)) => *o == kw,
            _ => false,
        }
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        let y = self.peek_kw(kw);
        if y {
            self.i += 1;
        }
        y
    }
    fn expect_op(&mut self, op: &str) -> Result<()> {
        if self.eat_kw(op) { Ok(()) } else { Err(DataError::BadRequest(format!("expected {op:?} in the expression{}", self.near()))) }
    }
    fn near(&self) -> String {
        match self.t.get(self.i) {
            Some(Tok::Num(s)) | Some(Tok::Ident(s)) => format!(", near {s}"),
            Some(Tok::Str(s)) => format!(", near \"{s}\""),
            Some(Tok::Quoted(s)) => format!(", near `{s}`"),
            Some(Tok::Op(o)) => format!(", near {o}"),
            None => ", at the end".to_string(),
        }
    }

    fn or(&mut self) -> Result<Expr> {
        let mut l = self.and()?;
        while self.eat_kw("or") {
            l = Expr::Bin(BinOp::Or, Box::new(l), Box::new(self.and()?));
        }
        Ok(l)
    }
    fn and(&mut self) -> Result<Expr> {
        let mut l = self.not()?;
        while self.eat_kw("and") {
            l = Expr::Bin(BinOp::And, Box::new(l), Box::new(self.not()?));
        }
        Ok(l)
    }
    fn not(&mut self) -> Result<Expr> {
        if self.eat_kw("not") { Ok(Expr::Not(Box::new(self.not()?))) } else { self.cmp() }
    }
    fn cmp(&mut self) -> Result<Expr> {
        let l = self.add()?;
        for (kw, op) in [("=", BinOp::Eq), ("!=", BinOp::Ne), ("<=", BinOp::Le), (">=", BinOp::Ge), ("<", BinOp::Lt), (">", BinOp::Gt)] {
            if matches!(self.t.get(self.i), Some(Tok::Op(o)) if *o == kw) {
                self.i += 1;
                return Ok(Expr::Bin(op, Box::new(l), Box::new(self.add()?)));
            }
        }
        if self.eat_kw("is") {
            let neg = self.eat_kw("not");
            if !self.eat_kw("null") {
                return Err(DataError::BadRequest(format!("expected null after is{}", self.near())));
            }
            return Ok(Expr::IsNull(Box::new(l), neg));
        }
        let neg = self.peek_kw("not") && matches!(self.t.get(self.i + 1), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("in"));
        if neg {
            self.i += 1;
        }
        if self.eat_kw("in") {
            self.expect_op("(")?;
            let mut items = vec![self.add()?];
            while self.eat_kw(",") {
                items.push(self.add()?);
            }
            self.expect_op(")")?;
            let e = Expr::In(Box::new(l), items);
            return Ok(if neg { Expr::Not(Box::new(e)) } else { e });
        }
        Ok(l)
    }
    fn add(&mut self) -> Result<Expr> {
        let mut l = self.mul()?;
        loop {
            let op = if self.eat_kw("+") {
                BinOp::Add
            } else if self.eat_kw("-") {
                BinOp::Sub
            } else {
                return Ok(l);
            };
            l = Expr::Bin(op, Box::new(l), Box::new(self.mul()?));
        }
    }
    fn mul(&mut self) -> Result<Expr> {
        let mut l = self.unary()?;
        loop {
            let op = if self.eat_kw("*") {
                BinOp::Mul
            } else if self.eat_kw("/") {
                BinOp::Div
            } else if self.eat_kw("%") {
                BinOp::Rem
            } else {
                return Ok(l);
            };
            l = Expr::Bin(op, Box::new(l), Box::new(self.unary()?));
        }
    }
    fn unary(&mut self) -> Result<Expr> {
        if self.eat_kw("-") { Ok(Expr::Neg(Box::new(self.unary()?))) } else { self.atom() }
    }
    fn atom(&mut self) -> Result<Expr> {
        let Some(t) = self.t.get(self.i).cloned() else { return Err(DataError::BadRequest("the expression ends too soon".into())) };
        self.i += 1;
        match t {
            Tok::Num(n) => {
                if n.contains(['.', 'e', 'E']) {
                    n.parse::<f64>().map(|f| Expr::Lit(Lit::Float(f))).map_err(|_| DataError::BadRequest(format!("{n} is not a number")))
                } else {
                    n.parse::<i64>().map(|f| Expr::Lit(Lit::Int(f))).or_else(|_| n.parse::<f64>().map(|f| Expr::Lit(Lit::Float(f)))).map_err(|_| DataError::BadRequest(format!("{n} is not a number")))
                }
            }
            Tok::Str(s) => Ok(Expr::Lit(Lit::Str(s))),
            Tok::Quoted(s) => Ok(Expr::Col(s)),
            Tok::Op("(") => {
                let e = self.or()?;
                self.expect_op(")")?;
                Ok(e)
            }
            Tok::Ident(name) => {
                let low = name.to_ascii_lowercase();
                if matches!(self.t.get(self.i), Some(Tok::Op("("))) {
                    self.i += 1;
                    let Some(&(f, lo, hi)) = FUNCS.iter().find(|f| f.0 == low) else {
                        return Err(DataError::BadRequest(format!("unknown function {name}; the functions are {}", FUNCS.iter().map(|f| f.0).collect::<Vec<_>>().join(", "))));
                    };
                    let mut args = Vec::new();
                    if !self.eat_kw(")") {
                        args.push(self.or()?);
                        while self.eat_kw(",") {
                            args.push(self.or()?);
                        }
                        self.expect_op(")")?;
                    }
                    if args.len() < lo || args.len() > hi {
                        return Err(DataError::BadRequest(format!("{f} takes {}{} argument(s), got {}", lo, if hi != lo { format!(" to {hi}") } else { String::new() }, args.len())));
                    }
                    return Ok(Expr::Call(f.to_string(), args));
                }
                Ok(match low.as_str() {
                    "true" => Expr::Lit(Lit::Bool(true)),
                    "false" => Expr::Lit(Lit::Bool(false)),
                    "null" | "none" => Expr::Lit(Lit::Null),
                    _ => Expr::Col(name),
                })
            }
            Tok::Op(o) => {
                self.i -= 1;
                Err(DataError::BadRequest(format!("unexpected {o} in the expression{}", self.near())))
            }
        }
    }
}

/// Parse an expression.
pub fn parse(src: &str) -> Result<Expr> {
    if src.trim().is_empty() {
        return Err(DataError::BadRequest("the expression is empty".into()));
    }
    let mut p = P { t: lex(src)?, i: 0 };
    let e = p.or()?;
    if p.i < p.t.len() {
        return Err(DataError::BadRequest(format!("cannot read the expression{} (use and / or between conditions, quote text, put odd column names in `backticks`)", p.near())));
    }
    Ok(e)
}

// ------------------------------------------------------------------ typing

/// The type of a sub-expression: a column type, or the type of a bare `null` (fits anything).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    T(DType),
    Null,
}

fn find<'a>(cols: &'a [Column], name: &str) -> Result<&'a Column> {
    cols.iter().find(|c| c.name == name).or_else(|| cols.iter().find(|c| c.name.eq_ignore_ascii_case(name))).ok_or_else(|| no_column(cols, name))
}

pub fn no_column(cols: &[Column], name: &str) -> DataError {
    let list: Vec<&str> = cols.iter().map(|c| c.name.as_str()).take(30).collect();
    DataError::NoSuchColumn(format!("there is no column {name:?}; the columns are: {}", list.join(", ")))
}

/// The real name of a column (case-insensitive when only one matches).
pub fn resolve(cols: &[Column], name: &str) -> Result<String> {
    find(cols, name).map(|c| c.name.clone())
}

/// Check an expression against the columns and give its type.
pub fn check(e: &Expr, cols: &[Column]) -> Result<Ty> {
    let bad = |m: String| Err(DataError::BadRequest(m));
    Ok(match e {
        Expr::Lit(Lit::Null) => Ty::Null,
        Expr::Lit(Lit::Bool(_)) => Ty::T(DType::Bool),
        Expr::Lit(Lit::Int(_)) => Ty::T(DType::Int),
        Expr::Lit(Lit::Float(_)) => Ty::T(DType::Float),
        Expr::Lit(Lit::Str(_)) => Ty::T(DType::Str),
        Expr::Col(n) => Ty::T(find(cols, n)?.dtype),
        Expr::Neg(x) => match check(x, cols)? {
            Ty::T(t) if t.numeric() => Ty::T(t),
            _ => return bad("a minus sign needs a number".into()),
        },
        Expr::Not(x) => match check(x, cols)? {
            Ty::T(DType::Bool) => Ty::T(DType::Bool),
            _ => return bad("not needs a true/false condition".into()),
        },
        Expr::IsNull(x, _) => {
            check(x, cols)?;
            Ty::T(DType::Bool)
        }
        Expr::In(x, items) => {
            let a = check(x, cols)?;
            for i in items {
                let b = check(i, cols)?;
                compatible(a, b)?;
            }
            Ty::T(DType::Bool)
        }
        Expr::Bin(op, a, b) => {
            let (ta, tb) = (check(a, cols)?, check(b, cols)?);
            match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => match (ta, tb) {
                    (Ty::T(x), Ty::T(y)) if x.numeric() && y.numeric() => {
                        if x == DType::Int && y == DType::Int && *op != BinOp::Div { Ty::T(DType::Int) } else { Ty::T(DType::Float) }
                    }
                    (Ty::Null, Ty::T(y)) | (Ty::T(y), Ty::Null) if y.numeric() => Ty::T(y),
                    _ => return bad(format!("{} needs numbers on both sides (use concat(a, b) to join text)", op_name(*op))),
                },
                BinOp::And | BinOp::Or => match (ta, tb) {
                    (Ty::T(DType::Bool), Ty::T(DType::Bool)) => Ty::T(DType::Bool),
                    _ => return bad("and / or need true/false conditions on both sides (put each comparison in its own part: a > 1 and b < 2)".into()),
                },
                _ => {
                    compatible(ta, tb)?;
                    Ty::T(DType::Bool)
                }
            }
        }
        Expr::Call(f, args) => {
            let t: Vec<Ty> = args.iter().map(|a| check(a, cols)).collect::<Result<_>>()?;
            let want = |i: usize, d: DType| -> Result<()> {
                match t[i] {
                    Ty::T(x) if x == d || (d == DType::Float && x.numeric()) => Ok(()),
                    Ty::Null => Ok(()),
                    _ => Err(DataError::BadRequest(format!("{f}: argument {} must be {}", i + 1, if d == DType::Str { "text" } else { "a number" }))),
                }
            };
            match f.as_str() {
                "contains" | "starts_with" | "ends_with" => {
                    want(0, DType::Str)?;
                    want(1, DType::Str)?;
                    Ty::T(DType::Bool)
                }
                "lower" | "upper" | "trim" => {
                    want(0, DType::Str)?;
                    Ty::T(DType::Str)
                }
                "len" => {
                    want(0, DType::Str)?;
                    Ty::T(DType::Int)
                }
                "abs" => {
                    want(0, DType::Float)?;
                    t[0]
                }
                "round" | "floor" | "ceil" => {
                    want(0, DType::Float)?;
                    if t.len() == 2 {
                        want(1, DType::Float)?;
                    }
                    Ty::T(DType::Float)
                }
                "is_null" => Ty::T(DType::Bool),
                "concat" => Ty::T(DType::Str),
                "coalesce" => {
                    let mut out = Ty::Null;
                    for x in &t {
                        compatible(out, *x)?;
                        if out == Ty::Null || (out == Ty::T(DType::Int) && *x == Ty::T(DType::Float)) {
                            out = *x;
                        }
                    }
                    out
                }
                _ => return bad(format!("unknown function {f}")),
            }
        }
    })
}

fn op_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        _ => "%",
    }
}

fn compatible(a: Ty, b: Ty) -> Result<()> {
    match (a, b) {
        (Ty::Null, _) | (_, Ty::Null) => Ok(()),
        (Ty::T(x), Ty::T(y)) if x == y || (x.numeric() && y.numeric()) => Ok(()),
        (Ty::T(x), Ty::T(y)) => Err(DataError::BadRequest(format!("cannot compare {} with {} (quote text: \"abc\"; numbers have no quotes)", x.name(), y.name()))),
    }
}

/// The column names an expression uses, as written.
pub fn columns_used(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Lit(_) => {}
        Expr::Col(n) => {
            if !out.contains(n) {
                out.push(n.clone());
            }
        }
        Expr::Neg(x) | Expr::Not(x) | Expr::IsNull(x, _) => columns_used(x, out),
        Expr::Bin(_, a, b) => {
            columns_used(a, out);
            columns_used(b, out);
        }
        Expr::Call(_, a) => a.iter().for_each(|x| columns_used(x, out)),
        Expr::In(x, l) => {
            columns_used(x, out);
            l.iter().for_each(|x| columns_used(x, out));
        }
    }
}

// ------------------------------------------------------------------ evaluation (the built-in engine)

/// A value while evaluating.
#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

impl V {
    fn f(&self) -> Option<f64> {
        match self {
            V::Int(i) => Some(*i as f64),
            V::Float(f) => Some(*f),
            _ => None,
        }
    }
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            V::Null => serde_json::Value::Null,
            V::Bool(b) => (*b).into(),
            V::Int(i) => (*i).into(),
            V::Float(f) => serde_json::Number::from_f64(*f).map_or(serde_json::Value::Null, serde_json::Value::Number),
            V::Str(s) => s.clone().into(),
        }
    }
    pub fn text(&self) -> String {
        match self {
            V::Null => String::new(),
            V::Bool(b) => b.to_string(),
            V::Int(i) => i.to_string(),
            V::Float(f) => fmt_float(*f),
            V::Str(s) => s.clone(),
        }
    }
}

pub fn fmt_float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 { format!("{f:.1}") } else { format!("{f}") }
}

/// Evaluate with `col(name)` giving the row's value of a column. Run [`check`] first: this assumes the types fit.
pub fn eval(e: &Expr, col: &dyn Fn(&str) -> V) -> V {
    match e {
        Expr::Lit(Lit::Null) => V::Null,
        Expr::Lit(Lit::Bool(b)) => V::Bool(*b),
        Expr::Lit(Lit::Int(i)) => V::Int(*i),
        Expr::Lit(Lit::Float(f)) => V::Float(*f),
        Expr::Lit(Lit::Str(s)) => V::Str(s.clone()),
        Expr::Col(n) => col(n),
        Expr::Neg(x) => match eval(x, col) {
            V::Int(i) => V::Int(i.wrapping_neg()),
            V::Float(f) => V::Float(-f),
            _ => V::Null,
        },
        Expr::Not(x) => match eval(x, col) {
            V::Bool(b) => V::Bool(!b),
            _ => V::Null,
        },
        Expr::IsNull(x, neg) => V::Bool((eval(x, col) == V::Null) != *neg),
        Expr::In(x, items) => {
            let v = eval(x, col);
            if v == V::Null {
                return V::Null;
            }
            V::Bool(items.iter().any(|i| equals(&v, &eval(i, col)) == Some(true)))
        }
        Expr::Bin(op, a, b) => {
            match op {
                // three-valued logic: false and null is false, true or null is true
                BinOp::And => {
                    let (x, y) = (eval(a, col), eval(b, col));
                    return match (x, y) {
                        (V::Bool(false), _) | (_, V::Bool(false)) => V::Bool(false),
                        (V::Bool(true), V::Bool(true)) => V::Bool(true),
                        _ => V::Null,
                    };
                }
                BinOp::Or => {
                    let (x, y) = (eval(a, col), eval(b, col));
                    return match (x, y) {
                        (V::Bool(true), _) | (_, V::Bool(true)) => V::Bool(true),
                        (V::Bool(false), V::Bool(false)) => V::Bool(false),
                        _ => V::Null,
                    };
                }
                _ => {}
            }
            let (x, y) = (eval(a, col), eval(b, col));
            if x == V::Null || y == V::Null {
                return V::Null;
            }
            match op {
                BinOp::Eq => equals(&x, &y).map_or(V::Null, V::Bool),
                BinOp::Ne => equals(&x, &y).map_or(V::Null, |b| V::Bool(!b)),
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => match order(&x, &y) {
                    Some(o) => V::Bool(match op {
                        BinOp::Lt => o.is_lt(),
                        BinOp::Le => o.is_le(),
                        BinOp::Gt => o.is_gt(),
                        _ => o.is_ge(),
                    }),
                    None => V::Null,
                },
                _ => arith(*op, &x, &y),
            }
        }
        Expr::Call(f, args) => {
            let v: Vec<V> = args.iter().map(|a| eval(a, col)).collect();
            call(f, &v)
        }
    }
}

fn equals(a: &V, b: &V) -> Option<bool> {
    match (a, b) {
        (V::Str(x), V::Str(y)) => Some(x == y),
        (V::Bool(x), V::Bool(y)) => Some(x == y),
        _ => Some(a.f()? == b.f()?),
    }
}

fn order(a: &V, b: &V) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (V::Str(x), V::Str(y)) => Some(x.cmp(y)),
        (V::Bool(x), V::Bool(y)) => Some(x.cmp(y)),
        (V::Int(x), V::Int(y)) => Some(x.cmp(y)),
        _ => a.f()?.partial_cmp(&b.f()?),
    }
}

fn arith(op: BinOp, x: &V, y: &V) -> V {
    if let (V::Int(a), V::Int(b)) = (x, y) {
        match op {
            BinOp::Add => return a.checked_add(*b).map_or(V::Null, V::Int),
            BinOp::Sub => return a.checked_sub(*b).map_or(V::Null, V::Int),
            BinOp::Mul => return a.checked_mul(*b).map_or(V::Null, V::Int),
            BinOp::Rem => return a.checked_rem(*b).map_or(V::Null, V::Int),
            _ => {}
        }
    }
    let (Some(a), Some(b)) = (x.f(), y.f()) else { return V::Null };
    let r = match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => a / b,
        _ => a % b,
    };
    if r.is_finite() { V::Float(r) } else { V::Null }
}

fn call(f: &str, v: &[V]) -> V {
    let s = |i: usize| -> Option<&str> {
        match v.get(i) {
            Some(V::Str(s)) => Some(s.as_str()),
            _ => None,
        }
    };
    match f {
        "contains" => match (s(0), s(1)) {
            (Some(a), Some(b)) => V::Bool(a.to_lowercase().contains(&b.to_lowercase())),
            _ => V::Null,
        },
        "starts_with" => match (s(0), s(1)) {
            (Some(a), Some(b)) => V::Bool(a.to_lowercase().starts_with(&b.to_lowercase())),
            _ => V::Null,
        },
        "ends_with" => match (s(0), s(1)) {
            (Some(a), Some(b)) => V::Bool(a.to_lowercase().ends_with(&b.to_lowercase())),
            _ => V::Null,
        },
        "lower" => s(0).map_or(V::Null, |a| V::Str(a.to_lowercase())),
        "upper" => s(0).map_or(V::Null, |a| V::Str(a.to_uppercase())),
        "trim" => s(0).map_or(V::Null, |a| V::Str(a.trim().to_string())),
        "len" => s(0).map_or(V::Null, |a| V::Int(a.chars().count() as i64)),
        "abs" => match &v[0] {
            V::Int(i) => V::Int(i.wrapping_abs()),
            V::Float(f) => V::Float(f.abs()),
            _ => V::Null,
        },
        "floor" => v[0].f().map_or(V::Null, |x| V::Float(x.floor())),
        "ceil" => v[0].f().map_or(V::Null, |x| V::Float(x.ceil())),
        "round" => {
            let n = v.get(1).and_then(V::f).unwrap_or(0.0).clamp(0.0, 12.0) as i32;
            let m = 10f64.powi(n);
            v[0].f().map_or(V::Null, |x| V::Float((x * m).round() / m))
        }
        "is_null" => V::Bool(v[0] == V::Null),
        "concat" => V::Str(v.iter().map(V::text).collect()),
        "coalesce" => v.iter().find(|x| **x != V::Null).cloned().unwrap_or(V::Null),
        _ => V::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols() -> Vec<Column> {
        vec![
            Column { name: "price".into(), dtype: DType::Float },
            Column { name: "qty".into(), dtype: DType::Int },
            Column { name: "name".into(), dtype: DType::Str },
            Column { name: "unit price".into(), dtype: DType::Float },
        ]
    }

    fn run(src: &str, row: &[(&str, V)]) -> V {
        let e = parse(src).unwrap();
        check(&e, &cols()).unwrap();
        eval(&e, &|n| row.iter().find(|r| r.0 == n).map(|r| r.1.clone()).unwrap_or(V::Null))
    }

    #[test]
    fn parses_and_evaluates() {
        let row = [("price", V::Float(12.5)), ("qty", V::Int(3)), ("name", V::Str("Green Tea".into())), ("unit price", V::Float(2.0))];
        assert_eq!(run("price > 10 and contains(name, \"tea\")", &row), V::Bool(true));
        assert_eq!(run("`unit price` * qty", &row), V::Float(6.0));
        assert_eq!(run("[unit price] * qty + 1", &row), V::Float(7.0));
        assert_eq!(run("qty * 2 - 1", &row), V::Int(5));
        assert_eq!(run("qty in (1, 3) and not (price < 5)", &row), V::Bool(true));
        assert_eq!(run("name not in ('a', 'b')", &row), V::Bool(true));
        assert_eq!(run("round(price / 3, 2)", &row), V::Float(4.17));
        assert_eq!(run("name is not null", &row), V::Bool(true));
        assert_eq!(run("concat(name, '!')", &row), V::Str("Green Tea!".into()));
        assert_eq!(run("price >= 12.5 && qty != 4", &row), V::Bool(true));
    }

    #[test]
    fn nulls_follow_three_valued_logic() {
        let row = [("price", V::Null), ("qty", V::Int(1))];
        assert_eq!(run("price > 1", &row), V::Null);
        assert_eq!(run("price > 1 and qty > 5", &row), V::Bool(false));
        assert_eq!(run("price > 1 or qty = 1", &row), V::Bool(true));
        assert_eq!(run("price is null", &row), V::Bool(true));
        assert_eq!(run("coalesce(price, 0)", &row), V::Int(0));
    }

    #[test]
    fn mistakes_are_explained() {
        let msg = |s: &str| match parse(s).and_then(|e| check(&e, &cols())) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("{s} should fail"),
        };
        assert!(msg("prce > 1").contains("no column \"prce\"") && msg("prce > 1").contains("price"));
        assert!(msg("name > 1").contains("cannot compare str with int"));
        assert!(msg("price > 1 and").contains("ends too soon"));
        assert!(msg("price > ").contains("ends too soon"));
        assert!(msg("foo(price)").contains("unknown function foo"));
        assert!(msg("name + 1").contains("needs numbers"));
        assert!(msg("\"abc").contains("never closed"));
        assert!(msg("price > 1 qty").contains("cannot read"));
        assert!(msg("round()").contains("takes 1 to 2"));
        assert!(msg("price and qty").contains("true/false"));
    }

    #[test]
    fn column_names_match_case_insensitively() {
        assert_eq!(resolve(&cols(), "PRICE").unwrap(), "price");
        assert!(resolve(&cols(), "zzz").is_err());
    }
}
