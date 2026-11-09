//! Style sheets, read the way a browser reads them but only as far as a reading browser can use them: rules with
//! selectors and declarations, `@media` conditions evaluated for one fixed screen, `@import`, comments, `!important`,
//! colours and lengths. Everything unknown is skipped, never an error: one bad rule must not lose the rest of a sheet.

use crate::domtree::{parse_selectors, Selector};

/// A selector for a style sheet: the lenient parser used by scripts also accepts `> a` and `a >`, which are errors here.
fn sheet_selectors(s: &str) -> Result<Vec<Selector>, String> {
    // each selector of a list stands on its own: an unsupported one drops only itself
    let mut out = Vec::new();
    for part in split_top(s, b',') {
        let t = part.trim();
        if t.is_empty() || t.starts_with(['>', '+', '~']) || t.ends_with(['>', '+', '~']) {
            continue;
        }
        if let Ok(mut v) = parse_selectors(t) {
            out.append(&mut v);
        }
    }
    if out.is_empty() {
        Err("no usable selector".into())
    } else {
        Ok(out)
    }
}

/// The screen the page is styled for. Media queries are answered from this.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
    /// The person asked for less motion (`prefers-reduced-motion: reduce`) / more contrast (`prefers-contrast: more`).
    pub reduce_motion: bool,
    pub more_contrast: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport { width: 1280.0, height: 800.0, reduce_motion: false, more_contrast: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decl {
    pub prop: String,
    pub value: String,
    pub important: bool,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub decls: Vec<Decl>,
    /// Position among all rules of the page (later wins a tie).
    pub order: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub rules: Vec<Rule>,
    /// Addresses named by `@import`, as written.
    pub imports: Vec<String>,
}

const MAX_RULES: usize = 40_000;
const MAX_DECLS: usize = 60;

/// Remove `/* … */` comments (not inside strings).
fn strip_comments(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let (mut i, mut quote) = (0, 0u8);
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            out.push(c);
            if c == b'\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
            out.push(c);
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 1;
            out.push(b' ');
        } else {
            out.push(c);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Index of the byte that closes the block opened at `open` (a `{`), or the end of the text.
fn matching_brace(s: &str, open: usize) -> usize {
    let b = s.as_bytes();
    let (mut depth, mut quote) = (0i32, 0u8);
    let mut i = open;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    b.len()
}

/// Split at top-level `;` (not inside parentheses or strings).
pub(crate) fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let b = s.as_bytes();
    let (mut depth, mut quote, mut start) = (0i32, 0u8, 0usize);
    let mut out = Vec::new();
    for (i, &c) in b.iter().enumerate() {
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
            continue;
        }
        match c {
            b'"' | b'\'' => quote = c,
            b'(' => depth += 1,
            b')' => depth -= 1,
            c if c == sep && depth <= 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

pub fn parse_decls(src: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    for part in split_top(src, b';') {
        let Some((p, v)) = part.split_once(':') else { continue };
        let prop = p.trim().to_ascii_lowercase();
        let mut value = v.trim().to_string();
        if prop.is_empty() || value.is_empty() || !prop.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            continue;
        }
        let mut important = false;
        let lower = value.to_ascii_lowercase();
        if let Some(i) = lower.rfind("!important") {
            if lower[i + 10..].trim().is_empty() {
                important = true;
                value = value[..i].trim().to_string();
            }
        }
        // custom properties keep their case; every other value is compared in lower case later
        out.push(Decl { prop, value, important });
        if out.len() >= MAX_DECLS {
            break;
        }
    }
    out
}

/// Read a style sheet. `order` is where its first rule stands among the page's rules.
pub fn parse_sheet(src: &str, vp: &Viewport, order: &mut u32) -> Sheet {
    let text = strip_comments(src);
    let mut sheet = Sheet::default();
    parse_into(&text, vp, order, &mut sheet, 0);
    sheet
}

fn parse_into(text: &str, vp: &Viewport, order: &mut u32, sheet: &mut Sheet, depth: u32) {
    let mut i = 0;
    let b = text.as_bytes();
    while i < b.len() && sheet.rules.len() < MAX_RULES {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'}' || b[i] == b';') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if b[i] == b'@' {
            // an at-rule: ends at `;` or after its block
            let semi = text[i..].find(';').map(|k| i + k);
            let open = text[i..].find('{').map(|k| i + k);
            match (semi, open) {
                (Some(s), o) if o.is_none_or(|o| s < o) => {
                    let rule = text[i..s].trim();
                    if let Some(rest) = rule.strip_prefix("@import") {
                        if let Some(u) = import_target(rest) {
                            if depth == 0 && sheet.imports.len() < 8 {
                                let (url, cond) = u;
                                if cond.is_empty() || media_matches(&cond, vp) {
                                    sheet.imports.push(url);
                                }
                            }
                        }
                    }
                    i = s + 1;
                }
                (_, Some(o)) => {
                    let head = text[i..o].trim().to_ascii_lowercase();
                    let end = matching_brace(text, o);
                    let inner = &text[(o + 1).min(text.len())..end.min(text.len())];
                    if let Some(q) = head.strip_prefix("@media") {
                        if depth < 4 && media_matches(q, vp) {
                            parse_into(inner, vp, order, sheet, depth + 1);
                        }
                    } else if (head.starts_with("@supports") || head.starts_with("@layer") || head.starts_with("@container")) && depth < 4 {
                        // `@supports` is taken as true, `@layer` as plain rules; container queries are not answered
                        if !head.starts_with("@container") && !head.starts_with("@supports not") {
                            parse_into(inner, vp, order, sheet, depth + 1);
                        }
                    }
                    // @font-face, @keyframes, @page, @property… carry nothing a reading browser uses
                    i = end + 1;
                }
                _ => break,
            }
            continue;
        }
        let Some(open) = text[i..].find('{').map(|k| i + k) else { break };
        let end = matching_brace(text, open);
        let sel = text[i..open].trim();
        let body = &text[(open + 1).min(text.len())..end.min(text.len())];
        i = end + 1;
        // nested rules (CSS nesting) are not supported: the declarations before the first nested block are kept
        let body = body.split('{').next().map(|_| body).unwrap_or(body);
        let decls = parse_decls(&strip_nested(body));
        if decls.is_empty() {
            continue;
        }
        if let Ok(selectors) = sheet_selectors(sel) {
            *order += 1;
            sheet.rules.push(Rule { selectors, decls, order: *order });
        }
    }
}

/// Drop `{ … }` blocks (nested rules) from a declaration list.
fn strip_nested(body: &str) -> String {
    if !body.contains('{') {
        return body.to_string();
    }
    let mut out = String::new();
    let mut depth = 0;
    let mut chunk_start = 0;
    for (i, c) in body.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    // the text before `{` back to the previous `;` is a nested selector
                    let before = &body[chunk_start..i];
                    let keep = before.rfind(';').map_or(0, |k| k + 1);
                    out.push_str(&before[..keep]);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    chunk_start = i + 1;
                }
            }
            _ => {}
        }
    }
    if depth == 0 {
        out.push_str(&body[chunk_start..]);
    }
    out
}

/// `url("x.css") screen` or `"x.css"`: the address and the media condition after it.
fn import_target(rest: &str) -> Option<(String, String)> {
    let r = rest.trim();
    let (url, tail) = if let Some(u) = r.strip_prefix("url(") {
        let end = u.find(')')?;
        (u[..end].trim().trim_matches(['"', '\'']).to_string(), u[end + 1..].trim().to_string())
    } else {
        let q = r.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let end = r[1..].find(q)?;
        (r[1..1 + end].to_string(), r[end + 2..].trim().to_string())
    };
    (!url.is_empty()).then_some((url, tail))
}

// ---------------------------------------------------------------- media queries

pub fn media_matches(query: &str, vp: &Viewport) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    split_top(&q, b',').into_iter().any(|one| media_one(one.trim(), vp))
}

fn media_one(q: &str, vp: &Viewport) -> bool {
    let mut q = q.trim();
    let mut negate = false;
    if let Some(r) = q.strip_prefix("not ") {
        negate = true;
        q = r.trim();
    } else if let Some(r) = q.strip_prefix("only ") {
        q = r.trim();
    }
    let mut ok = true;
    let mut rest = q;
    // a media type first
    if !rest.starts_with('(') {
        let end = rest.find(|c: char| c.is_whitespace() || c == '(').unwrap_or(rest.len());
        let (ty, tail) = rest.split_at(end);
        ok &= matches!(ty, "all" | "screen" | "");
        rest = tail.trim();
        rest = rest.strip_prefix("and").map_or(rest, str::trim);
    }
    while let Some(start) = rest.find('(') {
        let mut depth = 0;
        let mut end = None;
        for (i, c) in rest[start..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { return false };
        ok &= media_feature(rest[start + 1..end].trim(), vp);
        rest = rest[end + 1..].trim();
        rest = rest.strip_prefix("and").map_or(rest, str::trim);
    }
    ok != negate
}

fn media_len(v: &str, vp: &Viewport) -> Option<f32> {
    match parse_length(v.trim(), 16.0, 16.0, vp)? {
        Length::Px(p) => Some(p),
        _ => None,
    }
}

fn media_feature(f: &str, vp: &Viewport) -> bool {
    // range syntax: (width >= 600px), (400px <= width <= 700px)
    for op in ["<=", ">=", "<", ">", "="] {
        if f.contains(op) && !f.contains(':') {
            let parts: Vec<&str> = f.split(op).map(str::trim).collect();
            if parts.len() == 3 && parts[1] != "" && matches!(parts[1], "width" | "height") {
                // 600px <= width <= 1200px
                return media_feature(&format!("{} {op} {}", parts[0], parts[1]), vp) && media_feature(&format!("{} {op} {}", parts[1], parts[2]), vp);
            }
            if parts.len() == 2 {
                let (a, b) = (parts[0], parts[1]);
                let (dim, val, flip) = if matches!(a, "width" | "height") { (a, b, false) } else { (b, a, true) };
                let cur = if dim == "width" { vp.width } else { vp.height };
                let Some(v) = media_len(val, vp) else { return false };
                let ord = cur.partial_cmp(&v).unwrap_or(std::cmp::Ordering::Equal);
                let ord = if flip { ord.reverse() } else { ord };
                return match op {
                    "<=" => ord.is_le(),
                    ">=" => ord.is_ge(),
                    "<" => ord.is_lt(),
                    ">" => ord.is_gt(),
                    _ => ord.is_eq(),
                };
            }
        }
    }
    let (name, val) = match f.split_once(':') {
        Some((n, v)) => (n.trim(), v.trim()),
        None => (f, ""),
    };
    match name {
        "min-width" => media_len(val, vp).is_some_and(|v| vp.width >= v),
        "max-width" => media_len(val, vp).is_some_and(|v| vp.width <= v),
        "min-height" => media_len(val, vp).is_some_and(|v| vp.height >= v),
        "max-height" => media_len(val, vp).is_some_and(|v| vp.height <= v),
        "width" => media_len(val, vp).is_some_and(|v| (vp.width - v).abs() < 0.5),
        "orientation" => (val == "landscape") == (vp.width >= vp.height),
        "prefers-color-scheme" => val == "light",
        "prefers-reduced-motion" => match val {
            "reduce" => vp.reduce_motion,
            "no-preference" => !vp.reduce_motion,
            _ => false,
        },
        "prefers-contrast" => match val {
            "more" | "high" => vp.more_contrast,
            "no-preference" => !vp.more_contrast,
            _ => false,
        },
        "forced-colors" => match val {
            "active" => vp.more_contrast,
            "none" => !vp.more_contrast,
            _ => false,
        },
        "hover" | "any-hover" => val == "hover" || val.is_empty(),
        "pointer" | "any-pointer" => val == "fine" || val.is_empty(),
        "min-resolution" | "-webkit-min-device-pixel-ratio" | "min-device-pixel-ratio" => {
            let n: f32 = val.trim_end_matches("dppx").trim_end_matches('x').parse().unwrap_or(2.0);
            n <= 1.0
        }
        "color" | "monochrome" if val.is_empty() => name == "color",
        "display-mode" => val == "browser",
        _ => false,
    }
}

// ---------------------------------------------------------------- values

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Px(f32),
    Percent(f32),
    Auto,
    /// `calc()` mixing a percentage and pixels: `pct`% of the container plus `px` pixels. Resolved at layout time.
    Calc { pct: f32, px: f32 },
    /// `min()` / `max()` of two such sums (`min(100%, 600px)`), resolved at layout time. `a` and `b` are `[pct, px]`.
    Pick { a: [f32; 2], b: [f32; 2], max: bool },
}

impl Length {
    /// The length against a container size `of` (for percentages); pixel parts are scaled by `scale`. `None` for `auto`.
    pub fn resolve(self, of: f32, scale: f32) -> Option<f32> {
        let sum = |v: [f32; 2]| of * v[0] / 100.0 + v[1] * scale;
        match self {
            Length::Px(p) => Some(p * scale),
            Length::Percent(p) => Some(of * p / 100.0),
            Length::Auto => None,
            Length::Calc { pct, px } => Some(sum([pct, px])),
            Length::Pick { a, b, max } => Some(if max { sum(a).max(sum(b)) } else { sum(a).min(sum(b)) }),
        }
    }

    /// The pixel part only (percentages of an unknown container count as nothing): for margins and paddings.
    pub fn px_part(self) -> f32 {
        match self {
            Length::Px(p) => p,
            Length::Calc { px, .. } => px,
            Length::Pick { a, b, max } => if max { a[1].max(b[1]) } else { a[1].min(b[1]) },
            _ => 0.0,
        }
    }

    fn pair(self) -> Option<[f32; 2]> {
        match self {
            Length::Px(p) => Some([0.0, p]),
            Length::Percent(p) => Some([p, 0.0]),
            Length::Calc { pct, px } => Some([pct, px]),
            _ => None,
        }
    }

    fn from_pair(v: [f32; 2]) -> Length {
        if v[0] == 0.0 {
            Length::Px(v[1])
        } else if v[1] == 0.0 {
            Length::Percent(v[0])
        } else {
            Length::Calc { pct: v[0], px: v[1] }
        }
    }
}

/// A length, resolving `em`, `rem`, `vw`, `vh` and `calc()`, `min()`, `max()`, `clamp()`. `font` is the element's
/// font size and `root` the page's, both in pixels.
pub fn parse_length(v: &str, font: f32, root: f32, vp: &Viewport) -> Option<Length> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("auto") {
        return Some(Length::Auto);
    }
    let lower = v.to_ascii_lowercase();
    let lower = lower.strip_prefix("-webkit-").or_else(|| lower.strip_prefix("-moz-")).unwrap_or(&lower);
    if let Some(inner) = lower.strip_prefix("calc(").and_then(|r| r.strip_suffix(')')) {
        let mut p = CalcParser { s: inner.as_bytes(), i: 0, font, root, vp };
        let r = p.expr()?;
        p.ws();
        if p.i != p.s.len() || r.num {
            return None;
        }
        return Some(Length::from_pair([r.pct, r.px]));
    }
    for (name, is_max) in [("min(", false), ("max(", true)] {
        if let Some(inner) = lower.strip_prefix(name).and_then(|r| r.strip_suffix(')')) {
            let mut best: Option<Length> = None;
            for p in split_top(inner, b',') {
                let l = parse_length(p, font, root, vp)?;
                best = Some(match best {
                    None => l,
                    Some(Length::Px(a)) if matches!(l, Length::Px(_)) => {
                        let Length::Px(b) = l else { unreachable!() };
                        Length::Px(if is_max { a.max(b) } else { a.min(b) })
                    }
                    Some(a) => match (a.pair(), l.pair()) {
                        (Some(x), Some(y)) => Length::Pick { a: x, b: y, max: is_max },
                        _ => a,
                    },
                });
            }
            return best;
        }
    }
    if let Some(inner) = lower.strip_prefix("clamp(").and_then(|r| r.strip_suffix(')')) {
        let p: Vec<&str> = split_top(inner, b',');
        if p.len() == 3 {
            let (lo, mid, hi) = (parse_length(p[0], font, root, vp)?, parse_length(p[1], font, root, vp)?, parse_length(p[2], font, root, vp)?);
            if let (Length::Px(lo), Length::Px(mid), Length::Px(hi)) = (lo, mid, hi) {
                return Some(Length::Px(mid.clamp(lo, hi.max(lo))));
            }
            // mixed units: the preferred value capped by the maximum (the minimum is dropped)
            return Some(match (mid.pair(), hi.pair()) {
                (Some(a), Some(b)) => Length::Pick { a, b, max: false },
                _ => mid,
            });
        }
        return None;
    }
    if lower.parse::<f32>().is_ok_and(|n| n == 0.0) {
        return Some(Length::Px(0.0));
    }
    let split = lower.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+')))?;
    let (num, unit) = if split == 0 { return None } else { lower.split_at(split) };
    let n: f32 = num.parse().ok()?;
    Some(match unit {
        "px" => Length::Px(n),
        "em" => Length::Px(n * font),
        "rem" => Length::Px(n * root),
        "%" => Length::Percent(n),
        "pt" => Length::Px(n * 96.0 / 72.0),
        "pc" => Length::Px(n * 16.0),
        "in" => Length::Px(n * 96.0),
        "cm" => Length::Px(n * 96.0 / 2.54),
        "mm" => Length::Px(n * 96.0 / 25.4),
        "vw" => Length::Px(n * vp.width / 100.0),
        "vh" | "dvh" | "svh" | "lvh" => Length::Px(n * vp.height / 100.0),
        "vmin" => Length::Px(n * vp.width.min(vp.height) / 100.0),
        "vmax" => Length::Px(n * vp.width.max(vp.height) / 100.0),
        "ex" => Length::Px(n * font * 0.5),
        "ch" => Length::Px(n * font * 0.5),
        _ => return None,
    })
}

/// A `calc()` value: percent and pixel parts, or a plain number (`num`).
#[derive(Clone, Copy)]
struct CalcVal {
    pct: f32,
    px: f32,
    num: bool,
}

struct CalcParser<'a> {
    s: &'a [u8],
    i: usize,
    font: f32,
    root: f32,
    vp: &'a Viewport,
}

impl CalcParser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    /// term (('+' | '-') term)*
    fn expr(&mut self) -> Option<CalcVal> {
        let mut acc = self.term()?;
        loop {
            self.ws();
            let op = match self.s.get(self.i) {
                Some(b'+') => 1.0,
                Some(b'-') => -1.0,
                _ => return Some(acc),
            };
            self.i += 1;
            let t = self.term()?;
            if acc.num != t.num {
                return None;
            }
            acc = CalcVal { pct: acc.pct + op * t.pct, px: acc.px + op * t.px, num: acc.num };
        }
    }

    /// factor (('*' | '/') factor)*, where at most one side of a product has a unit
    fn term(&mut self) -> Option<CalcVal> {
        let mut acc = self.factor()?;
        loop {
            self.ws();
            let op = match self.s.get(self.i) {
                Some(b'*') => b'*',
                Some(b'/') => b'/',
                _ => return Some(acc),
            };
            self.i += 1;
            let f = self.factor()?;
            acc = if op == b'*' {
                match (acc.num, f.num) {
                    (_, true) => CalcVal { pct: acc.pct * f.px, px: acc.px * f.px, num: acc.num },
                    (true, false) => CalcVal { pct: f.pct * acc.px, px: f.px * acc.px, num: false },
                    _ => return None,
                }
            } else if f.num && f.px != 0.0 {
                CalcVal { pct: acc.pct / f.px, px: acc.px / f.px, num: acc.num }
            } else {
                return None;
            };
        }
    }

    fn factor(&mut self) -> Option<CalcVal> {
        self.ws();
        if self.s.get(self.i) == Some(&b'(') {
            self.i += 1;
            let v = self.expr()?;
            self.ws();
            if self.s.get(self.i) != Some(&b')') {
                return None;
            }
            self.i += 1;
            return Some(v);
        }
        let start = self.i;
        // a signed number or dimension, or a function call (`min(...)`, nested `calc(...)`)
        if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
            self.i += 1;
        }
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'%' | b'_')) {
            self.i += 1;
        }
        if self.s.get(self.i) == Some(&b'(') {
            let mut depth = 0;
            while let Some(&c) = self.s.get(self.i) {
                self.i += 1;
                match c {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
        let atom = std::str::from_utf8(&self.s[start..self.i]).ok()?;
        if atom.is_empty() {
            return None;
        }
        if let Ok(n) = atom.parse::<f32>() {
            return Some(CalcVal { pct: 0.0, px: n, num: true });
        }
        let [pct, px] = parse_length(atom, self.font, self.root, self.vp)?.pair()?;
        Some(CalcVal { pct, px, num: false })
    }
}

pub type Rgba = [u8; 4];

/// A colour: `#rgb`, `#rrggbb`, `#rgba`, `#rrggbbaa`, `rgb()`, `rgba()`, `hsl()`, `hsla()`, a name, `transparent`.
/// `currentcolor` is not handled here (the caller knows the current colour).
pub fn parse_color(v: &str) -> Option<Rgba> {
    let v = v.trim().to_ascii_lowercase();
    // light-dark(a, b): this reader shows pages in the light scheme
    if let Some(inner) = v.strip_prefix("light-dark(").and_then(|r| r.strip_suffix(')')) {
        return parse_color(split_top(inner, b',').first()?);
    }
    if let Some(h) = v.strip_prefix('#') {
        if !h.is_ascii() {
            return None;
        }
        let hex = |s: &str| u8::from_str_radix(s, 16).ok();
        let dup = |c: char| hex(&format!("{c}{c}"));
        return match h.len() {
            3 | 4 => {
                let c: Vec<char> = h.chars().collect();
                Some([dup(c[0])?, dup(c[1])?, dup(c[2])?, if c.len() == 4 { dup(c[3])? } else { 255 }])
            }
            6 | 8 => Some([hex(&h[0..2])?, hex(&h[2..4])?, hex(&h[4..6])?, if h.len() == 8 { hex(&h[6..8])? } else { 255 }]),
            _ => None,
        };
    }
    for f in ["rgba(", "rgb(", "hsla(", "hsl("] {
        if let Some(inner) = v.strip_prefix(f).and_then(|r| r.strip_suffix(')')) {
            let parts: Vec<&str> = inner.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|p| !p.is_empty()).collect();
            if parts.len() < 3 {
                return None;
            }
            let alpha = parts.get(3).map_or(Some(255), |a| {
                let n: f32 = a.trim_end_matches('%').parse().ok()?;
                Some(((if a.ends_with('%') { n / 100.0 } else { n }).clamp(0.0, 1.0) * 255.0).round() as u8)
            })?;
            if f.starts_with("rgb") {
                let ch = |s: &str| -> Option<u8> {
                    let n: f32 = s.trim_end_matches('%').parse().ok()?;
                    Some((if s.ends_with('%') { n * 2.55 } else { n }).clamp(0.0, 255.0).round() as u8)
                };
                return Some([ch(parts[0])?, ch(parts[1])?, ch(parts[2])?, alpha]);
            }
            let h: f32 = parts[0].trim_end_matches("deg").parse().ok()?;
            let sat: f32 = parts[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
            let li: f32 = parts[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
            let (r, g, b) = hsl_to_rgb(h, sat.clamp(0.0, 1.0), li.clamp(0.0, 1.0));
            return Some([r, g, b, alpha]);
        }
    }
    if v == "transparent" {
        return Some([0, 0, 0, 0]);
    }
    NAMED.binary_search_by(|(n, _)| (*n).cmp(v.as_str())).ok().map(|i| {
        let c = NAMED[i].1;
        [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]
    })
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let f = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (f(r), f(g), f(b))
}

/// The named colours, sorted for binary search.
const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

/// A `linear-gradient()` or `radial-gradient()`: the angle in CSS degrees (0 is up, 90 to the right, 180 down, the
/// default) and the colour stops with their places along the line (0 to 1).
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub angle: f32,
    pub radial: bool,
    pub stops: Vec<(f32, Rgba)>,
}

fn angle_of(w: &str) -> Option<f32> {
    let w = w.trim();
    let num = |suffix: &str| w.strip_suffix(suffix).and_then(|n| n.trim().parse::<f32>().ok());
    num("deg").or_else(|| num("turn").map(|t| t * 360.0)).or_else(|| num("rad").map(|r| r.to_degrees())).or_else(|| num("grad").map(|g| g * 0.9))
}

/// The first gradient in a `background` value. Colour hints and pixel stop positions are ignored (stops without a
/// percentage are spread evenly).
pub fn parse_gradient(v: &str) -> Option<Gradient> {
    let lower = v.to_ascii_lowercase();
    let (radial, at) = match (lower.find("linear-gradient("), lower.find("radial-gradient(")) {
        (Some(a), Some(b)) if b < a => (true, b),
        (Some(a), _) => (false, a),
        (None, Some(b)) => (true, b),
        _ => return None,
    };
    let rest = &lower[at + 16..];
    // the matching close parenthesis
    let mut depth = 1;
    let mut end = rest.len();
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let inner = &rest[..end];
    let mut angle = 180.0;
    let mut stops: Vec<(Option<f32>, Rgba)> = Vec::new();
    for (i, part) in split_top(inner, b',').into_iter().enumerate() {
        let part = part.trim();
        let ws: Vec<&str> = split_top(part, b' ').into_iter().filter(|w| !w.is_empty()).collect();
        if i == 0 && !ws.is_empty() {
            if ws[0] == "to" {
                let (mut up, mut down, mut left, mut right) = (false, false, false, false);
                for w in &ws[1..] {
                    match *w {
                        "top" => up = true,
                        "bottom" => down = true,
                        "left" => left = true,
                        "right" => right = true,
                        _ => {}
                    }
                }
                angle = match (up, down, left, right) {
                    (true, _, _, true) => 45.0,
                    (_, true, _, true) => 135.0,
                    (_, true, true, _) => 225.0,
                    (true, _, true, _) => 315.0,
                    (true, ..) => 0.0,
                    (_, _, _, true) => 90.0,
                    (_, _, true, _) => 270.0,
                    _ => 180.0,
                };
                continue;
            }
            if let Some(a) = angle_of(ws[0]) {
                angle = a;
                continue;
            }
            if radial && parse_color(ws[0]).is_none() {
                // a shape, size or position ("circle at top", "ellipse closest-side")
                continue;
            }
        }
        let Some(c) = ws.iter().find_map(|w| parse_color(w)) else { continue };
        let pos = ws.iter().find_map(|w| w.strip_suffix('%').and_then(|n| n.parse::<f32>().ok())).map(|p| (p / 100.0).clamp(0.0, 1.0));
        stops.push((pos, c));
    }
    if stops.len() < 2 {
        return None;
    }
    let n = stops.len();
    // fill in the places that were not given: the ends, then evenly between the ones that were
    if stops[0].0.is_none() {
        stops[0].0 = Some(0.0);
    }
    if stops[n - 1].0.is_none() {
        stops[n - 1].0 = Some(1.0);
    }
    let mut i = 0;
    while i < n {
        if stops[i].0.is_some() {
            i += 1;
            continue;
        }
        let prev = stops[i - 1].0.unwrap_or(0.0);
        let mut j = i;
        while stops[j].0.is_none() {
            j += 1;
        }
        let next = stops[j].0.unwrap_or(1.0);
        for k in i..j {
            stops[k].0 = Some(prev + (next - prev) * (k - i + 1) as f32 / (j - i + 1) as f32);
        }
        i = j;
    }
    let mut last = 0.0f32;
    let stops = stops
        .into_iter()
        .map(|(p, c)| {
            last = last.max(p.unwrap_or(0.0));
            (last, c)
        })
        .collect();
    Some(Gradient { angle: angle.rem_euclid(360.0), radial, stops })
}

impl Gradient {
    /// The colour at `t` (0 to 1) along the line, blended between the two stops around it.
    pub fn at(&self, t: f32) -> Rgba {
        let first = self.stops[0];
        if t <= first.0 {
            return first.1;
        }
        for w in self.stops.windows(2) {
            let ((p0, c0), (p1, c1)) = (w[0], w[1]);
            if t <= p1 {
                let k = if p1 - p0 < 1e-4 { 1.0 } else { (t - p0) / (p1 - p0) };
                let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k).round().clamp(0.0, 255.0) as u8;
                return [mix(c0[0], c1[0]), mix(c0[1], c1[1]), mix(c0[2], c1[2]), mix(c0[3], c1[3])];
            }
        }
        self.stops[self.stops.len() - 1].1
    }

    /// The average of all stops: what a radial gradient is filled with, and what text on it is read against.
    pub fn mean(&self) -> Rgba {
        let n = self.stops.len() as u32;
        let avg = |i: usize| (self.stops.iter().map(|s| s.1[i] as u32).sum::<u32>() / n) as u8;
        [avg(0), avg(1), avg(2), avg(3)]
    }
}

/// One `box-shadow`: offset, blur, spread (pixels) and colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Rgba,
}

/// The first outer shadow of a `box-shadow` value (`inset` shadows and `none` give nothing).
pub fn parse_shadow(v: &str, font: f32, vp: &Viewport) -> Option<Shadow> {
    let lower = v.to_ascii_lowercase();
    for layer in split_top(&lower, b',') {
        let ws: Vec<&str> = split_top(layer.trim(), b' ').into_iter().filter(|w| !w.is_empty()).collect();
        if ws.is_empty() || ws.contains(&"inset") {
            continue;
        }
        let mut nums: Vec<f32> = Vec::new();
        let mut color = None;
        for w in &ws {
            if let Some(c) = parse_color(w) {
                color = Some(c);
            } else if let Some(Length::Px(p)) = parse_length(w, font, 16.0, vp) {
                nums.push(p);
            }
        }
        if nums.len() < 2 {
            continue;
        }
        let color = color.unwrap_or([0, 0, 0, 90]);
        return Some(Shadow { dx: nums[0], dy: nums[1], blur: nums.get(2).copied().unwrap_or(0.0).max(0.0), spread: nums.get(3).copied().unwrap_or(0.0), color });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> Viewport {
        Viewport { width: 1100.0, height: 800.0, ..Viewport::default() }
    }

    #[test]
    fn the_named_colours_are_sorted_so_the_search_works() {
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn colours_in_every_notation() {
        assert_eq!(parse_color("#fff"), Some([255, 255, 255, 255]));
        assert_eq!(parse_color("#336699"), Some([0x33, 0x66, 0x99, 255]));
        assert_eq!(parse_color("#33669980"), Some([0x33, 0x66, 0x99, 0x80]));
        assert_eq!(parse_color("#f008"), Some([255, 0, 0, 0x88]));
        assert_eq!(parse_color("rgb(10, 20, 30)"), Some([10, 20, 30, 255]));
        assert_eq!(parse_color("rgb(10 20 30 / 50%)"), Some([10, 20, 30, 128]));
        assert_eq!(parse_color("rgba(255,0,0,.5)"), Some([255, 0, 0, 128]));
        assert_eq!(parse_color("rgb(100%, 0%, 0%)"), Some([255, 0, 0, 255]));
        assert_eq!(parse_color("hsl(120, 100%, 50%)"), Some([0, 255, 0, 255]));
        assert_eq!(parse_color("RebeccaPurple"), Some([0x66, 0x33, 0x99, 255]));
        assert_eq!(parse_color("transparent"), Some([0, 0, 0, 0]));
        assert_eq!(parse_color("light-dark(#eee, #222)"), Some([0xee, 0xee, 0xee, 255]));
        for bad in ["", "#12", "#ggg", "rgb(1,2)", "notacolor", "var(--x)"] {
            assert_eq!(parse_color(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn lengths_resolve_units_and_calc() {
        let l = |s: &str| parse_length(s, 20.0, 16.0, &vp());
        assert_eq!(l("12px"), Some(Length::Px(12.0)));
        assert_eq!(l("1.5em"), Some(Length::Px(30.0)));
        assert_eq!(l("2rem"), Some(Length::Px(32.0)));
        assert_eq!(l("50%"), Some(Length::Percent(50.0)));
        assert_eq!(l("auto"), Some(Length::Auto));
        assert_eq!(l("10vw"), Some(Length::Px(110.0)));
        assert_eq!(l("calc(100px - 2em)"), Some(Length::Px(60.0)));
        assert_eq!(l("calc(2 * (10px + 5px))"), Some(Length::Px(30.0)));
        assert_eq!(l("min(100px, 50px)"), Some(Length::Px(50.0)));
        assert_eq!(l("clamp(10px, 5px, 20px)"), Some(Length::Px(10.0)));
        assert_eq!(l("calc(100% - 40px)"), Some(Length::Calc { pct: 100.0, px: -40.0 }));
        assert_eq!(l("calc(2rem + 4px)"), Some(Length::Px(36.0)));
        assert_eq!(l("calc((100% - 20px) / 2)"), Some(Length::Calc { pct: 50.0, px: -10.0 }));
        assert_eq!(l("calc(50% + (2 * 1em))"), Some(Length::Calc { pct: 50.0, px: 40.0 }));
        assert_eq!(l("calc(100% - calc(10px + 5px))"), Some(Length::Calc { pct: 100.0, px: -15.0 }));
        assert_eq!(l("calc(100%)"), Some(Length::Percent(100.0)));
        assert_eq!(l("calc(10px * 3)"), Some(Length::Px(30.0)));
        assert_eq!(l("calc(20em*-1)"), Some(Length::Px(-400.0)), "no spaces round the operator");
        assert_eq!(l("calc(10px + 3)"), None);
        assert_eq!(l("calc(10px / 0)"), None);
        assert_eq!(l("calc(100% - )"), None);
        assert_eq!(l("min(100%, 600px)"), Some(Length::Pick { a: [100.0, 0.0], b: [0.0, 600.0], max: false }));
        assert_eq!(l("max(50%, 300px)"), Some(Length::Pick { a: [50.0, 0.0], b: [0.0, 300.0], max: true }));
        assert_eq!(l("clamp(1px, 80%, 500px)"), Some(Length::Pick { a: [80.0, 0.0], b: [0.0, 500.0], max: false }));
        assert_eq!(Length::Calc { pct: 100.0, px: -40.0 }.resolve(500.0, 2.0), Some(420.0));
        assert_eq!(l("min(100%, 600px)").unwrap().resolve(500.0, 1.0), Some(500.0));
        assert_eq!(l("min(100%, 600px)").unwrap().resolve(900.0, 1.0), Some(600.0));
        assert_eq!(l("max(50%, 300px)").unwrap().resolve(900.0, 1.0), Some(450.0));
        assert_eq!(l("12"), None);
        assert_eq!(l("bold"), None);
        assert_eq!(l("5furlongs"), None);
    }

    #[test]
    fn rules_comments_important_and_bad_input() {
        let mut n = 0;
        let s = parse_sheet("/* c */ p, .a > b { color: red; margin:0 auto !important } ~~bad~~ { x } h1{font-size:2em;;} p{}", &vp(), &mut n);
        assert_eq!(s.rules.len(), 2, "{:#?}", s.rules.iter().map(|r| (r.selectors.len(), r.decls.clone())).collect::<Vec<_>>());
        assert_eq!(s.rules[0].selectors.len(), 2);
        assert_eq!(s.rules[0].decls[1], Decl { prop: "margin".into(), value: "0 auto".into(), important: true });
        assert!(s.rules[1].order > s.rules[0].order);
        // one bad selector in a list loses only itself
        let s = parse_sheet("a, ~bad, b { color: blue }", &vp(), &mut n);
        assert_eq!(s.rules[0].selectors.len(), 2);
        // values with semicolons in strings and urls survive
        let d = parse_decls("background: url(\"a;b.png\") red; content: \"x;y\"");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].value, "url(\"a;b.png\") red");
        // garbage and truncation never panic
        for junk in ["{{{{", "}}}}", "a{", "@media (", "@import", "a{b:c", "\u{0}\u{1}", "a{b:url(", "@media screen{a{b:c}"] {
            let _ = parse_sheet(junk, &vp(), &mut n);
        }
    }

    #[test]
    fn media_queries_follow_the_screen() {
        let m = |q: &str| media_matches(q, &vp());
        assert!(m("screen") && m("all") && m("") && m("only screen and (min-width: 600px)"));
        assert!(!m("print") && !m("(max-width: 600px)") && !m("screen and (max-width: 767px)"));
        assert!(m("(max-width: 1200px)") && m("(min-width: 1000px) and (max-width: 1200px)"));
        assert!(m("print, (min-width: 800px)"), "a comma means or");
        assert!(m("not print") && !m("not screen"));
        assert!(m("(prefers-color-scheme: light)") && !m("(prefers-color-scheme: dark)"));
        assert!(m("(width >= 600px)") && !m("(width < 600px)") && m("(600px <= width <= 1200px)"));
        assert!(m("(min-width: 40em)"), "40em is 640px");
        assert!(!m("(unknown-thing: 3)"));
        let mut n = 0;
        let s = parse_sheet("@media (max-width: 600px){a{color:red}} @media (min-width: 600px){b{color:blue}} @media print{c{color:green}}", &vp(), &mut n);
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].decls[0].value, "blue");
    }

    #[test]
    fn imports_and_other_at_rules() {
        let mut n = 0;
        let s = parse_sheet("@charset \"utf-8\"; @import url(a.css); @import \"b.css\" screen; @import url(p.css) print; @font-face{font-family:x;src:url(f.woff)} @keyframes k{from{a:b}to{a:c}} p{color:red}", &vp(), &mut n);
        assert_eq!(s.imports, vec!["a.css".to_string(), "b.css".to_string()]);
        assert_eq!(s.rules.len(), 1);
        // @supports is taken as true, @layer as plain rules
        let s = parse_sheet("@supports (display:grid){p{color:red}} @layer base{p{color:blue}}", &vp(), &mut n);
        assert_eq!(s.rules.len(), 2);
    }

    #[test]
    fn nested_blocks_do_not_swallow_the_declarations_around_them() {
        let mut n = 0;
        let s = parse_sheet("a { color: red; &:hover { color: blue } margin: 0 }", &vp(), &mut n);
        assert_eq!(s.rules.len(), 1);
        let names: Vec<_> = s.rules[0].decls.iter().map(|d| d.prop.as_str()).collect();
        assert_eq!(names, vec!["color", "margin"]);
    }
}

#[cfg(test)]
mod effect_tests {
    use super::*;

    #[test]
    fn gradients_parse_direction_and_stops() {
        let g = parse_gradient("linear-gradient(to right, #ff0000, rgba(0, 0, 255, 0.5) 50%, #00ff00)").expect("a gradient");
        assert_eq!(g.angle, 90.0);
        assert_eq!(g.stops.len(), 3);
        assert_eq!(g.stops[1].0, 0.5);
        assert_eq!(g.stops[1].1, [0, 0, 255, 128]);
        assert_eq!(g.stops[2].0, 1.0);
        let g = parse_gradient("linear-gradient(45deg, red 0%, blue 100%)").unwrap();
        assert_eq!(g.angle, 45.0);
        assert_eq!(g.at(0.0), [255, 0, 0, 255]);
        assert_eq!(g.at(1.0), [0, 0, 255, 255]);
        let mid = g.at(0.5);
        assert!(mid[0] > 120 && mid[0] < 135 && mid[2] > 120 && mid[2] < 135, "{mid:?}");
        assert!(parse_gradient("linear-gradient(red)").is_none());
        assert!(parse_gradient("url(a.png)").is_none());
        let r = parse_gradient("radial-gradient(circle at top, #fff, #000)").unwrap();
        assert!(r.radial);
        assert_eq!(r.stops.len(), 2);
    }

    #[test]
    fn shadows_parse_offsets_blur_and_colour() {
        let vp = Viewport::default();
        let s = parse_shadow("0 4px 12px 2px rgba(0,0,0,.25), 0 0 1px red", 16.0, &vp).expect("a shadow");
        assert_eq!((s.dx, s.dy, s.blur, s.spread), (0.0, 4.0, 12.0, 2.0));
        assert_eq!(s.color[3], 64);
        assert!(parse_shadow("inset 0 1px 2px #000", 16.0, &vp).is_none());
        assert!(parse_shadow("none", 16.0, &vp).is_none());
    }
}
