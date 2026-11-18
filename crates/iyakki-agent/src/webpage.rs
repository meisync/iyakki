//! Understanding a downloaded page without running it: the article text without the menus and banners, its headline, date
//! and author, the links that lead to articles, the "next page" links, the site's RSS/Atom feed, and what a page that
//! builds itself with JavaScript still says in its JSON-LD and OpenGraph tags. Also RSS/Atom parsing and the reasons a
//! page cannot be read ("403: it blocks automated readers").
//!
//! Everything here is pure: text in, structures out. The network side is in [`crate::webresearch`].

use std::collections::{HashMap, HashSet};

use iyakki_web::html::{self, NodeKind, Tree};
use iyakki_web::Url;
use serde_json::Value;

// ------------------------------------------------------------------ small helpers

/// One line of text: control characters and runs of white space become single spaces.
pub fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` characters, cut at a word end with an ellipsis.
pub fn shorten(s: &str, max: usize) -> String {
    let s = collapse(s);
    if s.chars().count() <= max {
        return s;
    }
    let cut: String = s.chars().take(max).collect();
    let cut = match cut.rfind(' ') {
        Some(i) if i > max / 2 => cut[..i].to_string(),
        _ => cut,
    };
    format!("{}…", cut.trim_end_matches([' ', ',', ';', ':', '-']))
}

/// Text with tags taken out and entities decoded (summaries in feeds and JSON-LD sometimes hold HTML).
pub fn strip_tags(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("<![CDATA[").and_then(|x| x.strip_suffix("]]>")).unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let text = collapse(&html::decode_entities(&out));
    // HTML that was sent escaped (`&lt;p&gt;`) becomes tags after decoding: take those out as well
    if s.contains("&lt;") && text.contains('<') && text.contains('>') {
        return strip_tags(&text);
    }
    text
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Whole days since 1970-01-01 of a date written `YYYY-MM-DD...`.
pub fn day_number(date: &str) -> Option<i64> {
    let b = date.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d) = (date.get(0..4)?.parse::<i64>().ok()?, date.get(5..7)?.parse::<i64>().ok()?, date.get(8..10)?.parse::<i64>().ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d))
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// `2026-10-13 21:31` from an ISO date or time ("2026-10-13T21:31:00Z") or an RFC 822 date ("Tue, 13 Oct 2026 21:31:00 GMT");
/// anything else is returned shortened, an empty string stays empty.
pub fn norm_date(s: &str) -> String {
    let s = collapse(s);
    let b = s.as_bytes();
    if b.len() >= 10 && b[4] == b'-' && b[7] == b'-' && day_number(&s).is_some() {
        let mut out = s[..10].to_string();
        if b.len() >= 16 && (b[10] == b'T' || b[10] == b' ') && b[13] == b':' {
            out.push(' ');
            out.push_str(&s[11..16]);
        }
        return out;
    }
    let words: Vec<&str> = s.split([' ', ',']).filter(|w| !w.is_empty()).collect();
    // [Tue] 13 Oct 2026 [21:31:00 GMT]
    let start = words.iter().position(|w| w.parse::<u32>().is_ok_and(|d| (1..=31).contains(&d))).unwrap_or(usize::MAX);
    if start != usize::MAX && words.len() > start + 2 {
        let mon = words[start + 1].to_ascii_lowercase();
        if let (Some(m), Ok(y), Ok(d)) = (MONTHS.iter().position(|x| mon.starts_with(x)), words[start + 2].parse::<u32>(), words[start].parse::<u32>()) {
            let hm = words.get(start + 3).filter(|t| t.len() >= 5 && t.as_bytes()[2] == b':').map(|t| format!(" {}", &t[..5])).unwrap_or_default();
            return format!("{y:04}-{:02}-{d:02}{hm}", m + 1);
        }
    }
    shorten(&s, 40)
}

/// `/2026/10/13/` or `/20261013-` in an address.
pub fn date_in_path(path: &str) -> Option<String> {
    let segs: Vec<&str> = path.split('/').collect();
    for w in segs.windows(3) {
        if let (Ok(y), Ok(m), Ok(d)) = (w[0].parse::<u32>(), w[1].parse::<u32>(), w[2].get(..2).unwrap_or("x").parse::<u32>()) {
            if (1990..2100).contains(&y) && w[0].len() == 4 && (1..=12).contains(&m) && (1..=31).contains(&d) {
                return Some(format!("{y:04}-{m:02}-{d:02}"));
            }
        }
    }
    None
}

/// The host without `www.`.
pub fn bare_host(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

/// The same site? (`news.example.com` and `example.com` are, `example.org` is not.)
pub fn same_site(a: &str, b: &str) -> bool {
    let (a, b) = (bare_host(a), bare_host(b));
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}")) || site_name(a) == site_name(b)
}

/// The registrable part, roughly: `news.bbc.co.uk` -> `bbc.co.uk`, `finance.yahoo.com` -> `yahoo.com`.
pub fn site_name(host: &str) -> String {
    let parts: Vec<&str> = host.split('.').collect();
    let n = parts.len();
    let second_level = n >= 3 && parts[n - 1].len() == 2 && matches!(parts[n - 2], "co" | "com" | "org" | "net" | "gov" | "ac" | "edu");
    let keep = if second_level { 3 } else { 2 };
    parts[n.saturating_sub(keep)..].join(".")
}

/// `host/path?query` of an address, no scheme (the form links are listed in).
pub fn short_url(u: &Url) -> String {
    format!("{}{}{}", u.host, if u.path == "/" { "".to_string() } else { u.path.clone() }, u.query.as_ref().map(|q| format!("?{q}")).unwrap_or_default())
}

// ------------------------------------------------------------------ the tree

fn attr_lc(t: &Tree, i: usize, name: &str) -> String {
    t.attr(i, name).unwrap_or("").to_ascii_lowercase()
}

const BLOCK: &[&str] = &[
    "p", "div", "li", "ul", "ol", "table", "tr", "blockquote", "pre", "h1", "h2", "h3", "h4", "h5", "h6", "section", "article", "dd", "dt", "dl", "figure", "figcaption", "details", "summary", "main", "header", "footer", "nav", "aside", "form", "td", "th", "thead", "tbody", "hgroup", "address", "fieldset",
];

fn is_block(tag: &str) -> bool {
    BLOCK.contains(&tag)
}

/// Sizes of every subtree, computed once: characters of text, characters inside links, does it contain a block element.
struct Stats {
    tlen: Vec<usize>,
    link: Vec<usize>,
    has_block: Vec<bool>,
}

fn stats(t: &Tree) -> Stats {
    let n = t.nodes.len();
    let mut order = Vec::with_capacity(n);
    let mut stack = vec![0usize];
    while let Some(i) = stack.pop() {
        order.push(i);
        stack.extend(t.nodes[i].children.iter().rev().copied());
    }
    let mut s = Stats { tlen: vec![0; n], link: vec![0; n], has_block: vec![false; n] };
    for &i in order.iter().rev() {
        match &t.nodes[i].kind {
            NodeKind::Text(x) => {
                s.tlen[i] = x.trim().chars().count();
            }
            NodeKind::Element { tag, .. } => {
                if tag == "a" {
                    s.link[i] = s.tlen[i];
                }
                if is_block(tag) {
                    let p = t.nodes[i].parent;
                    if p < n {
                        s.has_block[p] = true;
                    }
                }
            }
        }
        let p = t.nodes[i].parent;
        if p < n {
            s.tlen[p] += s.tlen[i];
            if t.tag(p) != Some("a") {
                s.link[p] += s.link[i];
            }
            if s.has_block[i] {
                s.has_block[p] = true;
            }
        }
    }
    // text directly under a link counts as link text
    for &i in order.iter() {
        if t.tag(i) == Some("a") {
            s.link[i] = s.tlen[i];
        }
    }
    s
}

const JUNK_WORDS: &[&str] = &[
    "cookie", "consent", "gdpr", "newsletter", "subscribe", "subscription", "paywall", "advert", "adslot", "ad-slot", "sponsor", "promo", "related", "recommend", "trending", "most-read", "mostread", "most_read", "social", "sharing", "share-", "-share", "comment", "breadcrumb", "sidebar", "widget", "popup", "modal", "overlay", "masthead", "outbrain", "taboola", "toolbar",
    "skip-link", "skiplink", "footer", "topnav", "navbar", "nav-", "-nav", "menu", "signin", "sign-in", "login", "byline-share", "ticker",
];
const JUNK_TOKENS: &[&str] = &["ad", "ads", "nav", "share", "tools", "advertisement", "banner", "cta", "promo"];

fn junk_class(t: &Tree, i: usize) -> bool {
    for name in ["class", "id"] {
        let v = attr_lc(t, i, name);
        if v.is_empty() {
            continue;
        }
        if v.split_whitespace().any(|tok| JUNK_TOKENS.contains(&tok) || JUNK_WORDS.iter().any(|w| tok.contains(w))) {
            return true;
        }
    }
    false
}

const BAD_ROLES: &[&str] = &["navigation", "banner", "contentinfo", "complementary", "dialog", "alertdialog", "search", "menu", "menubar", "alert"];

struct Cx<'a> {
    t: &'a Tree,
    st: &'a Stats,
    /// Text size of the part being read; a box is only dropped as clutter when it is clearly smaller than that.
    floor: usize,
}

impl Cx<'_> {
    fn junk(&self, i: usize, in_article: bool) -> bool {
        let t = self.t;
        let Some(tag) = t.tag(i) else { return false };
        if matches!(tag, "script" | "style" | "noscript" | "button" | "select" | "svg" | "iframe" | "dialog" | "template" | "canvas" | "object" | "embed" | "audio" | "video" | "picture" | "head" | "title" | "meta" | "link" | "input" | "textarea" | "label") {
            return true;
        }
        if t.attr(i, "hidden").is_some() || attr_lc(t, i, "aria-hidden") == "true" || attr_lc(t, i, "style").replace(' ', "").contains("display:none") {
            return true;
        }
        let small = self.st.tlen[i] * 10 < self.floor * 6;
        if matches!(tag, "nav" | "footer" | "aside" | "form") && small {
            return true;
        }
        if tag == "header" && !in_article && small {
            return true;
        }
        if BAD_ROLES.contains(&attr_lc(t, i, "role").as_str()) && small {
            return true;
        }
        if !matches!(tag, "html" | "body" | "main" | "article") && small && junk_class(t, i) {
            return true;
        }
        false
    }
}

const BOILERPLATE: &[&str] = &[
    "advertisement", "sign up for", "subscribe to", "follow us", "all rights reserved", "copyright ©", "read more", "click here", "cookie", "privacy policy", "terms of use", "skip to", "share this", "continue reading", "story continues", "sign in", "log in", "download the app", "add as a preferred source", "newsletter",
];

/// Style rules that ended up in the text (`.css-1a2b{font-family:...}`).
fn looks_css(line: &str) -> bool {
    let l = line.trim_start();
    ((l.starts_with('.') || l.starts_with('@') || l.starts_with('#') && !l.starts_with("# ")) && l.contains('{'))
        || (l.contains('{') && l.matches(';').count() >= 2 && l.contains(':'))
        || l.contains("googletag.")
        || l.contains("function(")
        || l.contains("window.")
        || l.contains("});")
}

fn boilerplate(line: &str) -> bool {
    let l = line.to_lowercase();
    l.chars().count() < 110 && BOILERPLATE.iter().any(|b| l.contains(b))
}

/// The readable text of a page.
#[derive(Debug, Default)]
pub struct Main {
    pub text: String,
    /// Paragraphs with real sentences in them (80 characters or more).
    pub paragraphs: usize,
    /// The text came out of an `<article>`/`itemprop=articleBody` part.
    pub from_article: bool,
}

struct Reader<'a> {
    cx: Cx<'a>,
    strict: bool,
    lines: Vec<String>,
    paragraphs: usize,
}

impl Reader<'_> {
    fn emit(&mut self, text: &str, tag: &str, len: usize, linked: usize) {
        let line = collapse(text);
        if line.is_empty() {
            return;
        }
        let n = line.chars().count();
        if looks_css(&line) {
            return;
        }
        if self.strict {
            let heavy = linked * 10 > len.max(1) * 6;
            let min = match tag {
                "li" => 30,
                "pre" => 1,
                "blockquote" | "p" | "div" | "td" | "dd" | "figcaption" | "section" | "" => 40,
                _ => 20,
            };
            if n < min || (heavy && n < 400) || boilerplate(&line) {
                return;
            }
            if tag == "li" && heavy {
                return;
            }
        } else if boilerplate(&line) && n < 60 {
            return;
        }
        if self.lines.last().is_some_and(|l| *l == line) {
            return;
        }
        if n >= 80 && matches!(tag, "p" | "div" | "blockquote" | "") {
            self.paragraphs += 1;
        }
        self.lines.push(if tag == "li" { format!("- {line}") } else { line });
    }

    fn walk(&mut self, i: usize, depth: usize, in_article: bool, root: bool) {
        let t = self.cx.t;
        let Some(tag) = t.tag(i).map(str::to_string) else { return };
        if depth > 190 || (!root && self.cx.junk(i, in_article)) {
            return;
        }
        let in_article = in_article || tag == "article";
        let st = self.cx.st;
        if matches!(tag.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
            let text = collapse(&t.text_of(i));
            if text.chars().count() >= if self.strict { 6 } else { 2 } && !boilerplate(&text) && !looks_css(&text) && self.lines.last() != Some(&format!("# {text}")) {
                self.lines.push(format!("# {text}"));
            }
            return;
        }
        if tag == "table" {
            let layout = st.tlen[i] > 3000 || self.nested(i, "table") || self.has_long_block(i);
            if !layout {
                let rows = self.rows(i);
                if !rows.is_empty() {
                    self.lines.extend(rows);
                    return;
                }
            }
        }
        if tag == "pre" {
            let text = t.text_of(i);
            let text = text.trim_end();
            if !text.trim().is_empty() {
                self.lines.push(text.to_string());
            }
            return;
        }
        if is_block(&tag) && !st.has_block[i] && !matches!(tag.as_str(), "ul" | "ol" | "table" | "tr" | "section" | "article" | "main") {
            let text = t.text_of(i);
            self.emit(&text, &tag, st.tlen[i], st.link[i]);
            return;
        }
        // a container: read its children, text that sits between block children is a paragraph of its own
        let mut run = String::new();
        let (mut run_len, mut run_link) = (0usize, 0usize);
        for &c in &t.nodes[i].children {
            match &t.nodes[c].kind {
                NodeKind::Text(x) => {
                    run.push_str(x);
                    run.push(' ');
                    run_len += x.trim().chars().count();
                }
                NodeKind::Element { tag: ct, .. } => {
                    let inline = !is_block(ct) && !st.has_block[c];
                    if inline {
                        if ct == "br" {
                            run.push(' ');
                        } else if !self.cx.junk(c, in_article) {
                            run.push_str(&t.text_of(c));
                            run.push(' ');
                            run_len += st.tlen[c];
                            run_link += st.link[c];
                        }
                    } else {
                        if !run.trim().is_empty() {
                            self.emit(&run, "", run_len, run_link);
                        }
                        run.clear();
                        run_len = 0;
                        run_link = 0;
                        self.walk(c, depth + 1, in_article, false);
                    }
                }
            }
        }
        if !run.trim().is_empty() {
            self.emit(&run, "", run_len, run_link);
        }
    }

    fn nested(&self, i: usize, tag: &str) -> bool {
        let t = self.cx.t;
        let mut todo: Vec<usize> = t.nodes[i].children.clone();
        while let Some(n) = todo.pop() {
            if t.tag(n) == Some(tag) {
                return true;
            }
            todo.extend(t.nodes[n].children.iter().copied());
        }
        false
    }

    fn has_long_block(&self, i: usize) -> bool {
        let t = self.cx.t;
        let mut todo: Vec<usize> = t.nodes[i].children.clone();
        while let Some(n) = todo.pop() {
            if matches!(t.tag(n), Some("p" | "div" | "ul" | "ol")) && self.cx.st.tlen[n] > 400 {
                return true;
            }
            todo.extend(t.nodes[n].children.iter().copied());
        }
        false
    }

    fn rows(&self, table: usize) -> Vec<String> {
        let t = self.cx.t;
        let mut rows = Vec::new();
        let mut todo = vec![table];
        let mut order = Vec::new();
        while let Some(n) = todo.pop() {
            if t.tag(n) == Some("tr") {
                order.push(n);
                continue;
            }
            todo.extend(t.nodes[n].children.iter().rev().copied());
        }
        for tr in order {
            let cells: Vec<String> = t.nodes[tr].children.iter().filter(|&&c| matches!(t.tag(c), Some("td" | "th"))).map(|&c| collapse(&t.text_of(c))).collect();
            if cells.iter().any(|c| !c.is_empty()) {
                rows.push(cells.join(" | "));
            }
        }
        rows
    }
}

fn find_all<'a>(t: &'a Tree, pred: impl Fn(usize) -> bool + 'a) -> impl Iterator<Item = usize> + 'a {
    (0..t.nodes.len()).filter(move |&i| pred(i))
}

/// The article of a page without the clutter around it.
pub fn main_text(t: &Tree) -> Main {
    let st = stats(t);
    let body = t.find("body").unwrap_or(0);
    // the part of the page that holds the story
    let mut root = None;
    let mut from_article = false;
    if let Some(i) = find_all(t, |i| attr_lc(t, i, "itemprop") == "articlebody").filter(|&i| st.tlen[i] >= 200).max_by_key(|&i| st.tlen[i]) {
        root = Some(i);
        from_article = true;
    } else if let Some(i) = find_all(t, |i| t.tag(i) == Some("article")).max_by_key(|&i| st.tlen[i]).filter(|&i| st.tlen[i] >= 600) {
        root = Some(i);
        from_article = true;
    } else if let Some(i) = find_all(t, |i| t.tag(i) == Some("main") || attr_lc(t, i, "role") == "main").max_by_key(|&i| st.tlen[i]).filter(|&i| st.tlen[i] >= 200) {
        root = Some(i);
    }
    let attempts: Vec<(usize, bool, bool)> = match root {
        Some(r) => vec![(r, true, from_article), (body, true, false), (body, false, false)],
        None => vec![(body, true, false), (body, false, false)],
    };
    let mut best = Main::default();
    for (r, strict, art) in attempts {
        let cx = Cx { t, st: &st, floor: st.tlen[r].max(1) };
        let mut rd = Reader { cx, strict, lines: Vec::new(), paragraphs: 0 };
        rd.walk(r, 0, art, true);
        let text = rd.lines.join("\n");
        let n = text.chars().count();
        if n > best.text.chars().count() || best.text.is_empty() {
            best = Main { text, paragraphs: rd.paragraphs, from_article: art };
        }
        if n >= 250 {
            break;
        }
    }
    best
}

// ------------------------------------------------------------------ metadata, JSON-LD

/// `<meta>` tags by lower-case name or property (the first of each).
fn metas(t: &Tree) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for i in 0..t.nodes.len() {
        if t.tag(i) == Some("meta") {
            let key = ["property", "name", "itemprop", "http-equiv"].iter().map(|k| attr_lc(t, i, k)).find(|k| !k.is_empty());
            if let (Some(k), Some(v)) = (key, t.attr(i, "content")) {
                let v = collapse(v);
                if !v.is_empty() {
                    m.entry(k).or_insert(v);
                }
            }
        }
    }
    m
}

/// What a page says about itself in JSON-LD.
#[derive(Debug, Default)]
pub struct Ld {
    pub article: bool,
    pub headline: String,
    pub description: String,
    pub body: String,
    pub date: String,
    pub author: String,
    /// `(name, url)` of the entries of an `ItemList`.
    pub items: Vec<(String, String)>,
}

fn ld_text(v: &Value) -> String {
    match v {
        Value::String(s) => strip_tags(s),
        Value::Array(a) => a.iter().map(ld_text).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", "),
        Value::Object(o) => o.get("name").map(ld_text).unwrap_or_default(),
        _ => String::new(),
    }
}

fn ld_collect(v: &Value, out: &mut Ld, depth: usize) {
    if depth > 6 {
        return;
    }
    match v {
        Value::Array(a) => a.iter().for_each(|x| ld_collect(x, out, depth + 1)),
        Value::Object(o) => {
            if let Some(g) = o.get("@graph") {
                ld_collect(g, out, depth + 1);
            }
            let ty = match o.get("@type") {
                Some(Value::String(s)) => s.to_lowercase(),
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ").to_lowercase(),
                _ => String::new(),
            };
            let s = |k: &str| o.get(k).map(ld_text).unwrap_or_default();
            if ty.contains("article") || ty.contains("posting") || ty.contains("report") {
                let body = s("articleBody");
                if !out.article || body.chars().count() > out.body.chars().count() {
                    out.article = true;
                    out.headline = [s("headline"), s("name")].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
                    out.description = s("description");
                    out.body = body;
                    out.date = [s("datePublished"), s("dateCreated"), s("dateModified")].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
                    out.author = s("author");
                }
            } else if ty.contains("webpage") || ty.contains("collectionpage") || ty.contains("newsmediaorganization") {
                if out.description.is_empty() {
                    out.description = s("description");
                }
                if out.headline.is_empty() && ty.contains("page") {
                    out.headline = s("name");
                }
            }
            if ty.contains("itemlist") {
                if let Some(Value::Array(items)) = o.get("itemListElement") {
                    for it in items {
                        let inner = it.get("item").unwrap_or(it);
                        let url = inner.get("url").or_else(|| inner.get("@id")).or_else(|| it.get("url")).and_then(Value::as_str).unwrap_or("");
                        let name = [inner.get("name"), inner.get("headline"), it.get("name")].into_iter().flatten().map(ld_text).find(|x| !x.is_empty()).unwrap_or_default();
                        if url.starts_with("http") && out.items.len() < 60 {
                            out.items.push((name, url.to_string()));
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// All `<script type="application/ld+json">` blocks of a page, read.
pub fn json_ld(raw: &str) -> Ld {
    let lower = raw.to_ascii_lowercase();
    let mut out = Ld::default();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<script") {
        let start = at + i;
        let Some(gt) = lower[start..].find('>').map(|g| start + g) else { break };
        let end = lower[gt..].find("</script").map_or(lower.len(), |e| gt + e);
        if lower[start..gt].contains("ld+json") {
            let body = raw[gt + 1..end].trim();
            let body = body.strip_prefix("<![CDATA[").map(|b| b.trim_end_matches("]]>")).unwrap_or(body);
            if let Ok(v) = serde_json::from_str::<Value>(body) {
                ld_collect(&v, &mut out, 0);
            }
        }
        at = end.max(gt) + 1;
        if at >= lower.len() {
            break;
        }
    }
    out
}

// ------------------------------------------------------------------ links

const SOCIAL: &[&str] = &["facebook.com", "twitter.com", "x.com", "instagram.com", "linkedin.com", "pinterest.com", "whatsapp.com", "wa.me", "t.me", "telegram.org", "tiktok.com", "threads.net", "reddit.com/submit", "doubleclick.net", "googleadservices.com", "outbrain.com", "taboola.com", "addtoany.com", "bsky.app", "mastodon.social"];
const NAV_TEXT: &[&str] = &[
    "skip to content", "skip to main content", "sign in", "log in", "login", "subscribe", "menu", "search", "home", "contact us", "about us", "about", "privacy policy", "privacy", "terms of use", "terms", "cookie settings", "cookies", "advertise", "careers", "help", "follow us", "newsletters", "newsletter", "rss", "podcasts", "watch live", "sign up", "register", "my account", "e-paper", "epaper", "more", "see all", "view all", "read more", "share", "comments", "feedback", "sitemap", "accessibility", "archive",
];

#[derive(Clone, Copy, Default)]
struct Ctx {
    nav: bool,
    article: bool,
    heading: bool,
    list: bool,
    main: bool,
}

/// A link on a page, scored by how much it looks like a story (higher is better).
#[derive(Clone, Debug)]
pub struct Link {
    pub text: String,
    pub url: Url,
    pub score: i32,
}

fn article_like_path(u: &Url) -> bool {
    let segs: Vec<&str> = u.path.split('/').filter(|s| !s.is_empty()).collect();
    let Some(last) = segs.last() else { return false };
    let hyphens = last.matches('-').count();
    let digits = last.chars().filter(|c| c.is_ascii_digit()).count();
    let ext = last.ends_with(".html") || last.ends_with(".htm") || last.ends_with(".cms") || last.ends_with(".ece") || last.ends_with(".php") || last.ends_with(".aspx");
    date_in_path(&u.path).is_some() || hyphens >= 3 || (segs.len() >= 2 && (hyphens >= 2 || digits >= 6 || ext)) || (digits >= 8 && segs.len() >= 2)
}

fn nav_ctx_class(t: &Tree, i: usize) -> bool {
    for name in ["class", "id"] {
        let v = attr_lc(t, i, name);
        if v.split_whitespace().any(|tok| matches!(tok, "nav" | "menu" | "footer" | "sidebar" | "breadcrumb" | "social" | "share" | "cookie" | "consent" | "newsletter" | "masthead" | "topbar" | "header-nav") || ["navbar", "nav-", "-nav", "menu", "footer", "social", "cookie", "consent", "breadcrumb", "sponsor", "advert", "skip"].iter().any(|w| tok.contains(w))) {
            return true;
        }
    }
    false
}

/// All the links of a page, best first (then in page order).
pub fn page_links(t: &Tree, base: &Url) -> Vec<Link> {
    let mut found: Vec<(usize, Link)> = Vec::new();
    let mut stack: Vec<(usize, Ctx)> = vec![(0, Ctx::default())];
    let mut order = 0usize;
    while let Some((i, cx)) = stack.pop() {
        let NodeKind::Element { tag, .. } = &t.nodes[i].kind else { continue };
        let mut c = cx;
        match tag.as_str() {
            "script" | "style" | "noscript" | "template" | "svg" => continue,
            "nav" | "footer" | "aside" => c.nav = true,
            "header" if !cx.article => c.nav = true,
            "article" => c.article = true,
            "main" => c.main = true,
            "h1" | "h2" | "h3" | "h4" => c.heading = true,
            "li" => c.list = true,
            _ => {}
        }
        if matches!(attr_lc(t, i, "role").as_str(), "navigation" | "banner" | "contentinfo" | "complementary") || (tag != "body" && tag != "html" && nav_ctx_class(t, i)) {
            c.nav = true;
        }
        if t.attr(i, "hidden").is_some() || attr_lc(t, i, "aria-hidden") == "true" {
            continue;
        }
        let cls = format!("{} {}", attr_lc(t, i, "class"), attr_lc(t, i, "id"));
        if cls.contains("headline") || cls.contains("title") || cls.contains("heading") {
            c.heading = true;
        }
        if tag == "a" {
            if let Some(href) = t.attr(i, "href") {
                order += 1;
                if let Some(l) = score_link(t, i, href, base, c) {
                    found.push((order, l));
                }
            }
            continue;
        }
        stack.extend(t.nodes[i].children.iter().rev().map(|&ch| (ch, c)));
    }
    // one entry per address: the best text and score
    let mut best: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<(usize, Link)> = Vec::new();
    for (o, l) in found {
        let key = l.url.to_string();
        match best.get(&key) {
            Some(&k) => {
                if l.score > out[k].1.score || (l.score == out[k].1.score && l.text.len() > out[k].1.text.len()) {
                    out[k].1 = Link { score: l.score.max(out[k].1.score), ..l };
                }
            }
            None => {
                best.insert(key, out.len());
                out.push((o, l));
            }
        }
    }
    out.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    out.into_iter().map(|x| x.1).collect()
}

fn score_link(t: &Tree, i: usize, href: &str, base: &Url, cx: Ctx) -> Option<Link> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') || href.starts_with("javascript:") || href.starts_with("mailto:") || href.starts_with("tel:") {
        return None;
    }
    let url = base.join(href).ok()?.without_fragment();
    if url == base.without_fragment() {
        return None;
    }
    let full = format!("{}{}", url.host, url.path);
    if SOCIAL.iter().any(|s| full.contains(s) || url.host.ends_with(s)) {
        return None;
    }
    let mut text = collapse(&t.text_of(i));
    if text.is_empty() {
        text = ["aria-label", "title"].iter().filter_map(|a| t.attr(i, a)).map(collapse).find(|s| !s.is_empty()).unwrap_or_default();
    }
    if text.is_empty() {
        // a picture link: its alt text
        let mut todo = t.nodes[i].children.clone();
        while let Some(n) = todo.pop() {
            if t.tag(n) == Some("img") {
                text = t.attr(n, "alt").map(collapse).unwrap_or_default();
                break;
            }
            todo.extend(t.nodes[n].children.iter().copied());
        }
    }
    let lc_text = text.to_lowercase();
    if text.chars().count() < 3 || NAV_TEXT.contains(&lc_text.as_str()) || ["sign up", "subscribe", "download the", "get the app", "follow us", "log in", "sign in"].iter().any(|p| lc_text.starts_with(p)) {
        return None;
    }
    let words = text.split_whitespace().count();
    let mut score = 0;
    if cx.heading {
        score += 4;
    }
    if cx.article || cx.main {
        score += 1;
    }
    if cx.list {
        score += 1;
    }
    if article_like_path(&url) {
        score += 3;
    }
    score += match words {
        0..=2 => -2,
        3 => 0,
        4..=5 => 1,
        _ => 2,
    };
    if cx.nav {
        score -= 6;
    }
    if same_site(&url.host, &base.host) {
        score += 1;
    } else {
        score -= 3;
    }
    let segs = url.path.split('/').filter(|s| !s.is_empty()).count();
    if segs <= 1 && !url.path.contains('-') && url.query.is_none() {
        score -= 3;
    }
    if url.path.ends_with(".jpg") || url.path.ends_with(".png") || url.path.ends_with(".pdf") || url.path.ends_with(".mp4") {
        score -= 5;
    }
    Some(Link { text: shorten(&text, 160), url, score })
}

/// A link that is probably a story, not a menu entry.
pub const ARTICLE_SCORE: i32 = 4;

// ------------------------------------------------------------------ pagination

fn page_number(u: &Url) -> Option<(u64, &'static str)> {
    if let Some(q) = &u.query {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if matches!(k, "page" | "p" | "pg" | "paged" | "pageno" | "pagenum") {
                    return v.parse().ok().map(|n| (n, "query"));
                }
            }
        }
    }
    let segs: Vec<&str> = u.path.split('/').filter(|s| !s.is_empty()).collect();
    for w in segs.windows(2) {
        if w[0] == "page" {
            return w[1].parse().ok().map(|n| (n, "path"));
        }
    }
    None
}

/// The address of page `n` when the current address already carries a page number (`?page=2`, `/page/2/`).
pub fn with_page(u: &Url, n: u64) -> Option<Url> {
    let (_, kind) = page_number(u)?;
    let mut out = u.clone();
    if kind == "query" {
        let q = u.query.as_deref()?;
        let parts: Vec<String> = q.split('&').map(|pair| match pair.split_once('=') {
            Some((k, _)) if matches!(k, "page" | "p" | "pg" | "paged" | "pageno" | "pagenum") => format!("{k}={n}"),
            _ => pair.to_string(),
        }).collect();
        out.query = Some(parts.join("&"));
    } else {
        let segs: Vec<String> = u.path.split('/').map(str::to_string).collect();
        let mut done = false;
        let mut v = segs.clone();
        for k in 0..segs.len().saturating_sub(1) {
            if !done && segs[k] == "page" && segs[k + 1].parse::<u64>().is_ok() {
                v[k + 1] = n.to_string();
                done = true;
            }
        }
        out.path = v.join("/");
    }
    Some(out)
}

#[derive(Debug, Default, Clone)]
pub struct Pager {
    /// "next page" / "older" links, best first.
    pub next: Vec<(String, Url)>,
    /// Numbered page links `(number, address)`, in order.
    pub numbered: Vec<(u64, Url)>,
}

const NEXT_WORDS: &[&str] = &["next", "next page", "next »", "next ›", "next →", "older", "older posts", "older stories", "older articles", "older news", "more stories", "more news", "next 10", "›", "»", "→", ">", ">>", "show more", "load more"];

fn in_pager(t: &Tree, mut i: usize) -> bool {
    for _ in 0..6 {
        let p = t.nodes[i].parent;
        if p >= t.nodes.len() {
            return false;
        }
        i = p;
        for name in ["class", "id", "aria-label", "role"] {
            let v = attr_lc(t, i, name);
            if v.contains("pagin") || v.contains("pager") || v.contains("paging") || v.contains("page-nav") || v.contains("pagenav") {
                return true;
            }
        }
    }
    false
}

/// The "next page" and numbered page links of a listing.
pub fn pagination(t: &Tree, base: &Url) -> Pager {
    let mut pg = Pager::default();
    let mut seen: HashSet<String> = HashSet::new();
    let current = page_number(base).map(|x| x.0).unwrap_or(1);
    // <link rel="next"> first: the site says so itself
    for i in 0..t.nodes.len() {
        if t.tag(i) == Some("link") && attr_lc(t, i, "rel").split_whitespace().any(|r| r == "next") {
            if let Some(u) = t.attr(i, "href").and_then(|h| base.join(h).ok()) {
                if seen.insert(u.to_string()) {
                    pg.next.push(("next".into(), u.without_fragment()));
                }
            }
        }
    }
    for i in 0..t.nodes.len() {
        if t.tag(i) != Some("a") {
            continue;
        }
        let Some(u) = t.attr(i, "href").and_then(|h| base.join(h).ok()).map(|u| u.without_fragment()) else { continue };
        if u == base.without_fragment() || !same_site(&u.host, &base.host) {
            continue;
        }
        let text = collapse(&t.text_of(i));
        let lc = text.to_lowercase();
        let aria = attr_lc(t, i, "aria-label");
        let rel = attr_lc(t, i, "rel");
        let pager = in_pager(t, i);
        let is_prev = lc.starts_with("prev") || lc.starts_with("newer") || aria.contains("prev") || rel.contains("prev") || matches!(lc.as_str(), "‹" | "«" | "<" | "<<" | "←");
        let is_next = !is_prev && (rel.split_whitespace().any(|r| r == "next") || NEXT_WORDS.contains(&lc.as_str()) || ((lc.starts_with("next") || lc.starts_with("older") || aria.contains("next")) && text.split_whitespace().count() <= 4) || (pager && attr_lc(t, i, "class").contains("next")));
        if is_next && !(matches!(lc.as_str(), ">" | "show more" | "load more") && !pager) {
            if seen.insert(u.to_string()) {
                pg.next.push((if text.is_empty() { "next".into() } else { shorten(&text, 40) }, u));
            }
            continue;
        }
        if text.len() <= 4 && !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) && (pager || page_number(&u).is_some()) {
            if let Ok(n) = text.parse::<u64>() {
                if n != current && !pg.numbered.iter().any(|(m, _)| *m == n) {
                    pg.numbered.push((n, u));
                }
            }
        }
    }
    pg.numbered.sort_by_key(|(n, _)| *n);
    pg.numbered.truncate(10);
    pg.next.truncate(3);
    // a numbered address without a "next" link: guess the following page from the numbers
    if pg.next.is_empty() {
        if let Some((_, u)) = pg.numbered.iter().find(|(n, _)| *n == current + 1) {
            pg.next.push((format!("page {}", current + 1), u.clone()));
        } else if page_number(base).is_some() {
            if let Some(u) = with_page(base, current + 1) {
                pg.next.push((format!("page {} (guessed from the address)", current + 1), u));
            }
        }
    }
    pg
}

// ------------------------------------------------------------------ the page

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One story or document.
    Article,
    /// A front page or section page: mostly links to stories.
    Listing,
    /// Nothing to read without running scripts.
    Shell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// The page's own text.
    Reader,
    /// `articleBody` from the page's JSON-LD.
    JsonLd,
    /// Only the title/description from `og:`/`twitter:`/`<meta>` tags.
    Meta,
    Nothing,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub url: Url,
    pub title: String,
    pub site: String,
    pub date: String,
    pub byline: String,
    pub description: String,
    pub text: String,
    pub via: Via,
    pub kind: Kind,
    pub paragraphs: usize,
    pub links: Vec<Link>,
    pub feeds: Vec<Url>,
    pub amp: Option<Url>,
    pub pager: Pager,
}

impl Page {
    /// The links that look like stories, on the same site, best first.
    pub fn stories(&self) -> Vec<&Link> {
        self.links.iter().filter(|l| l.score >= ARTICLE_SCORE && same_site(&l.url.host, &self.url.host)).collect()
    }
}

/// Read a downloaded page.
pub fn analyze(raw: &str, url: &Url) -> Page {
    let t = html::parse(raw);
    let m = metas(&t);
    let ld = json_ld(raw);
    let meta = |keys: &[&str]| keys.iter().find_map(|k| m.get(*k)).cloned().unwrap_or_default();
    let title_tag = t.find("title").map(|i| collapse(&t.text_of(i))).unwrap_or_default();
    let h1 = find_all(&t, |i| t.tag(i) == Some("h1")).map(|i| collapse(&t.text_of(i))).find(|s| s.chars().count() >= 8).unwrap_or_default();
    // the headline without the site's name behind it when the <title> only adds to it; otherwise the <title> as the page wrote it
    let tag_title = if !h1.is_empty() && title_tag.to_lowercase().contains(&h1.to_lowercase()) { h1 } else { title_tag };
    let title = [meta(&["og:title", "twitter:title"]), ld.headline.clone(), tag_title].into_iter().map(|s| strip_tags(&s)).find(|s| !s.is_empty()).unwrap_or_default();
    let description = [meta(&["og:description", "twitter:description", "description"]), ld.description.clone()].into_iter().map(|s| strip_tags(&s)).find(|s| !s.is_empty()).unwrap_or_default();
    let site = [meta(&["og:site_name", "application-name"])].into_iter().find(|s| !s.is_empty()).unwrap_or_else(|| bare_host(&url.host).to_string());
    let date = [
        ld.date.clone(),
        meta(&["article:published_time", "og:article:published_time", "date", "pubdate", "publishdate", "dc.date", "dc.date.issued", "parsely-pub-date", "sailthru.date", "datepublished"]),
        find_all(&t, |i| t.tag(i) == Some("time") && t.attr(i, "datetime").is_some()).filter_map(|i| t.attr(i, "datetime").map(str::to_string)).next().unwrap_or_default(),
        date_in_path(&url.path).unwrap_or_default(),
    ]
    .into_iter()
    .map(|d| norm_date(&d))
    .find(|d| !d.is_empty())
    .unwrap_or_default();
    let byline = [ld.author.clone(), meta(&["author", "article:author", "parsely-author", "byl", "dc.creator"])].into_iter().map(|s| strip_tags(&s)).find(|s| !s.is_empty() && !s.starts_with("http")).unwrap_or_default();

    let main = main_text(&t);
    let mut text = main.text.clone();
    let mut via = if text.chars().count() >= 40 { Via::Reader } else { Via::Nothing };
    let mut paragraphs = main.paragraphs;
    let n = text.chars().count();
    // a page that builds itself with scripts: the structured data still holds the story
    if n < 400 && ld.body.chars().count() > n {
        text = ld.body.chars().take(60_000).collect();
        via = Via::JsonLd;
        paragraphs = paragraphs.max(1);
    }
    if text.chars().count() < 120 && !description.is_empty() && via != Via::JsonLd {
        // the old reader view may still find something the walker skipped
        let doc = iyakki_web::doc::from_html(raw);
        let old = iyakki_web::reader::plain_text(&doc, 40_000);
        if old.chars().count() > text.chars().count().max(300) {
            text = old;
            via = Via::Reader;
        } else {
            text = description.clone();
            via = Via::Meta;
        }
    } else if text.trim().is_empty() {
        let doc = iyakki_web::doc::from_html(raw);
        let old = iyakki_web::reader::plain_text(&doc, 40_000);
        if !old.trim().is_empty() {
            text = old;
            via = Via::Reader;
        }
    }

    let mut links = page_links(&t, url);
    // JSON-LD item lists (some front pages are shells with the list inside)
    for (name, u) in &ld.items {
        if let Ok(u2) = url.join(u) {
            if !links.iter().any(|l| l.url == u2) && !name.is_empty() {
                links.push(Link { text: shorten(name, 160), url: u2.without_fragment(), score: ARTICLE_SCORE });
            }
        }
    }
    let story_links = links.iter().filter(|l| l.score >= ARTICLE_SCORE && same_site(&l.url.host, &url.host)).count();
    let og_article = m.get("og:type").is_some_and(|v| v.contains("article"));
    let kind = if via == Via::Nothing {
        if story_links >= 5 { Kind::Listing } else { Kind::Shell }
    } else if story_links >= 8 && main.text.lines().filter(|l| l.starts_with("# ")).count() >= 8 && !og_article {
        Kind::Listing
    } else if (og_article || ld.article || main.from_article) && paragraphs >= 2 {
        Kind::Article
    } else if story_links >= 8 && paragraphs < 5 {
        Kind::Listing
    } else if paragraphs >= 3 {
        Kind::Article
    } else if story_links >= 5 && paragraphs < 3 {
        Kind::Listing
    } else if via == Via::Meta {
        Kind::Shell
    } else {
        Kind::Article
    };

    let mut feeds = Vec::new();
    let mut amp = None;
    for i in 0..t.nodes.len() {
        if t.tag(i) != Some("link") {
            continue;
        }
        let rel = attr_lc(&t, i, "rel");
        let ty = attr_lc(&t, i, "type");
        let Some(href) = t.attr(i, "href").and_then(|h| url.join(h).ok()) else { continue };
        if rel.split_whitespace().any(|r| r == "alternate") && (ty.contains("rss") || ty.contains("atom")) {
            feeds.push(href.without_fragment());
        } else if rel.split_whitespace().any(|r| r == "amphtml") {
            amp = Some(href.without_fragment());
        }
    }
    let pager = pagination(&t, url);
    // a front page has no author and no date of its own (a `<time>` on it belongs to some teaser)
    let (date, byline) = if kind == Kind::Listing { (String::new(), String::new()) } else { (date, byline) };
    Page { url: url.clone(), title: shorten(&title, 240), site: shorten(&site, 60), date, byline: shorten(&byline, 120), description: shorten(&description, 400), text, via, kind, paragraphs, links, feeds, amp, pager }
}

/// Between 600 and `max` characters from the start of the text: whole paragraphs, headings last, cut at a sentence end.
pub fn excerpt(text: &str, max: usize) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with("# ") || l.chars().count() < 40 {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(l.trim_start_matches("- "));
        if out.chars().count() >= max {
            break;
        }
    }
    if out.is_empty() {
        out = collapse(text);
    }
    if out.chars().count() <= max {
        return out;
    }
    let cut: String = out.chars().take(max).collect();
    // end at the last sentence stop if that keeps most of it
    let stop = cut.rfind(['.', '!', '?']).filter(|&i| i > max * 6 / 10);
    match stop {
        Some(i) => cut[..=i].to_string(),
        None => shorten(&cut, max),
    }
}

// ------------------------------------------------------------------ feeds

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedItem {
    pub title: String,
    pub link: String,
    pub date: String,
    pub summary: String,
}

#[derive(Clone, Debug, Default)]
pub struct Feed {
    pub title: String,
    pub items: Vec<FeedItem>,
}

/// The content of the first `<name>` element in `s` (`None` when absent).
fn xml_tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let lower = s.to_ascii_lowercase();
    let open = format!("<{}", name.to_ascii_lowercase());
    let mut from = 0;
    while let Some(i) = lower[from..].find(&open) {
        let at = from + i;
        let after = lower.as_bytes().get(at + open.len()).copied();
        if matches!(after, Some(b'>' | b' ' | b'\n' | b'\t' | b'\r' | b'/')) {
            let gt = lower[at..].find('>')? + at;
            if lower.as_bytes()[gt - 1] == b'/' {
                return Some("");
            }
            let close = format!("</{}>", name.to_ascii_lowercase());
            let end = lower[gt..].find(&close).map(|e| gt + e)?;
            return Some(&s[gt + 1..end]);
        }
        from = at + open.len();
    }
    None
}

fn xml_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let i = lower.find(&format!("{name}="))?;
    let rest = &tag[i + name.len() + 1..];
    let q = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let end = rest[1..].find(q)?;
    Some(html::decode_entities(&rest[1..1 + end]))
}

/// `true` when the text is an RSS, Atom or RDF feed.
pub fn looks_like_feed(text: &str) -> bool {
    let head: String = text.chars().take(1500).collect::<String>().to_ascii_lowercase();
    head.contains("<rss") || head.contains("<feed") || head.contains("<rdf:rdf")
}

fn element_chunks<'a>(s: &'a str, name: &str) -> Vec<&'a str> {
    let lower = s.to_ascii_lowercase();
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find(&open) {
        let at = from + i;
        if !matches!(lower.as_bytes().get(at + open.len()), Some(b'>' | b' ' | b'\n' | b'\t' | b'\r')) {
            from = at + open.len();
            continue;
        }
        let end = lower[at..].find(&close).map_or(s.len(), |e| at + e);
        out.push(&s[at..end]);
        from = end.max(at + 1);
        if out.len() >= 200 {
            break;
        }
    }
    out
}

/// Parse an RSS 2.0, RSS 1.0 (RDF) or Atom feed. `None` when the text is not a feed.
pub fn parse_feed(xml: &str, base: &Url) -> Option<Feed> {
    if !looks_like_feed(xml) {
        return None;
    }
    let atom = xml.to_ascii_lowercase().contains("<feed");
    let chunks = element_chunks(xml, if atom { "entry" } else { "item" });
    let head_end = xml.to_ascii_lowercase().find(if atom { "<entry" } else { "<item" }).unwrap_or(xml.len());
    let title = xml_tag(&xml[..head_end], "title").map(strip_tags).unwrap_or_default();
    let mut items = Vec::new();
    for c in chunks {
        let title = xml_tag(c, "title").map(strip_tags).unwrap_or_default();
        let mut link = String::new();
        if atom {
            // <link rel="alternate" href="..."/> (the first link without rel is the page too)
            let lower = c.to_ascii_lowercase();
            let mut from = 0;
            while let Some(i) = lower[from..].find("<link") {
                let at = from + i;
                let end = lower[at..].find('>').map_or(lower.len(), |e| at + e);
                let tag = &c[at..end];
                let rel = xml_attr(tag, "rel").unwrap_or_else(|| "alternate".into());
                if rel == "alternate" {
                    if let Some(h) = xml_attr(tag, "href") {
                        link = h;
                        break;
                    }
                }
                from = end;
            }
        } else {
            link = xml_tag(c, "feedburner:origLink").or_else(|| xml_tag(c, "link")).map(|l| strip_tags(l)).unwrap_or_default();
            if link.is_empty() {
                link = xml_tag(c, "guid").map(strip_tags).filter(|g| g.starts_with("http")).unwrap_or_default();
            }
        }
        let link = base.join(link.trim()).map(|u| u.without_fragment().to_string()).unwrap_or_default();
        let date = ["pubDate", "published", "updated", "dc:date", "date"].iter().find_map(|k| xml_tag(c, k)).map(strip_tags).map(|d| norm_date(&d)).unwrap_or_default();
        let summary = ["description", "summary", "content:encoded", "content"].iter().find_map(|k| xml_tag(c, k)).map(strip_tags).map(|s| shorten(&s, 400)).unwrap_or_default();
        if !title.is_empty() || !link.is_empty() {
            items.push(FeedItem { title: shorten(&title, 200), link, date, summary });
        }
    }
    Some(Feed { title: shorten(&title, 120), items })
}

/// Feeds that are known to work for sites whose pages cannot be read or have no feed link, most specific first.
pub fn known_feeds(host: &str, path: &str) -> Vec<&'static str> {
    const TABLE: &[(&str, &str, &str)] = &[
        ("marketwatch.com", "", "https://feeds.content.dowjones.io/public/rss/mw_realtimeheadlines"),
        ("marketwatch.com", "", "https://feeds.marketwatch.com/marketwatch/topstories/"),
        ("cnbc.com", "market", "https://www.cnbc.com/id/10000664/device/rss/rss.html"),
        ("cnbc.com", "financ", "https://www.cnbc.com/id/10000664/device/rss/rss.html"),
        ("cnbc.com", "", "https://www.cnbc.com/id/100003114/device/rss/rss.html"),
        ("bbc.com", "business", "https://feeds.bbci.co.uk/news/business/rss.xml"),
        ("bbc.co.uk", "business", "https://feeds.bbci.co.uk/news/business/rss.xml"),
        ("bbc.com", "", "https://feeds.bbci.co.uk/news/rss.xml"),
        ("bbc.co.uk", "", "https://feeds.bbci.co.uk/news/rss.xml"),
        ("theguardian.com", "business", "https://www.theguardian.com/business/rss"),
        ("theguardian.com", "", "https://www.theguardian.com/world/rss"),
        ("thehindu.com", "market", "https://www.thehindu.com/business/markets/feeder/default.rss"),
        ("thehindu.com", "business", "https://www.thehindu.com/business/feeder/default.rss"),
        ("thehindu.com", "", "https://www.thehindu.com/news/feeder/default.rss"),
        ("economictimes.indiatimes.com", "stock", "https://economictimes.indiatimes.com/markets/stocks/rssfeeds/2146842.cms"),
        ("economictimes.indiatimes.com", "market", "https://economictimes.indiatimes.com/markets/stocks/rssfeeds/2146842.cms"),
        ("economictimes.indiatimes.com", "", "https://economictimes.indiatimes.com/rssfeedstopstories.cms"),
        ("timesofindia.indiatimes.com", "", "https://timesofindia.indiatimes.com/rssfeedstopstories.cms"),
        ("aljazeera.com", "", "https://www.aljazeera.com/xml/rss/all.xml"),
        ("npr.org", "", "https://feeds.npr.org/1001/rss.xml"),
        ("nytimes.com", "", "https://rss.nytimes.com/services/xml/rss/nyt/Business.xml"),
    ];
    let host = bare_host(host);
    let mut out: Vec<&'static str> = Vec::new();
    for (h, kw, url) in TABLE {
        if (host == *h || host.ends_with(&format!(".{h}"))) && (kw.is_empty() || path.to_lowercase().contains(kw)) && !out.contains(url) {
            out.push(url);
        }
    }
    out
}

// ------------------------------------------------------------------ why a page cannot be read

/// The plain reason for an error status.
pub fn status_reason(host: &str, status: u16, url: &str) -> String {
    match status {
        401 | 403 => format!("{host} answered HTTP {status}: the site blocks automated readers (or wants a sign-in). It cannot be read from here; try another source"),
        402 => format!("{host} answered HTTP 402: the page is behind a paywall; try another source"),
        404 | 410 => format!("{host} has no page at {url} (HTTP {status}); the address may be old or mistyped, search again for the current one"),
        407 | 511 => format!("{host} answered HTTP {status}: a network sign-in is required before the page can be read"),
        408 => format!("{host} answered HTTP 408: it timed out waiting; try again"),
        429 => format!("{host} answered HTTP 429: it is rate-limiting readers; wait a bit or try another source"),
        451 => format!("{host} answered HTTP 451: the page is not available in this region"),
        500..=599 => format!("{host} had a server error (HTTP {status}); try again later or use another source"),
        _ => format!("{host} answered HTTP {status} for {url}"),
    }
}

/// The reason a page that arrived says nothing: a check for robots, a request to switch on JavaScript, a consent form.
/// Looks at pages with little text only, so a story that merely mentions "captcha" is never refused.
pub fn wall_reason(host: &str, title: &str, text: &str, chars: usize) -> Option<String> {
    if chars > 600 {
        return None;
    }
    let l = format!("{title} {text}").to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| l.contains(w));
    if has(&["captcha", "verify you are human", "verify that you are human", "are you a robot", "unusual traffic", "access denied", "request blocked", "just a moment", "attention required", "checking your browser", "automated access", "bot detection", "pardon our interruption"]) {
        Some(format!("{host} showed a bot check instead of the page: it blocks automated readers. Try another source"))
    } else if has(&["enable javascript", "enable js", "javascript is disabled", "javascript is required", "turn on javascript", "requires javascript", "you need to enable javascript", "please enable cookies"]) {
        Some(format!("{host} needs JavaScript to show this page (web_fetch does not run scripts) or blocks automated readers. Try another source"))
    } else if has(&["before you continue", "we value your privacy", "accept all cookies", "manage cookie", "consent preferences"]) || host.starts_with("consent.") {
        Some(format!("{host} showed a cookie/consent page instead of the content. Try another source"))
    } else {
        None
    }
}

/// A transport error, in words.
pub fn net_reason(host: &str, err: &str) -> String {
    let e = err.to_lowercase();
    if e.contains("timed out") || e.contains("timeout") {
        format!("{host} did not answer in time")
    } else if e.contains("failed to lookup") || e.contains("dns") || e.contains("resolve") || e.contains("no such host") || e.contains("name or service") {
        format!("{host} could not be found (no such address, or no internet connection)")
    } else if e.contains("certificate") || e.contains("tls") || e.contains("ssl") {
        format!("{host}: the secure connection failed ({err})")
    } else if e.contains("connection refused") || e.contains("connect") {
        format!("{host} refused the connection ({err})")
    } else {
        err.to_string()
    }
}

// ------------------------------------------------------------------ near-duplicates

fn shingles(text: &str) -> HashSet<u64> {
    let words: Vec<String> = text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 1).map(str::to_string).collect();
    let mut out = HashSet::new();
    let k = 4;
    if words.len() < k {
        for w in &words {
            out.insert(fnv(w));
        }
        return out;
    }
    for w in words.windows(k) {
        out.insert(fnv(&w.join(" ")));
    }
    out
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// How alike two texts are, 0 to 1 (the share of the shorter text's four-word runs that the other one has too).
pub fn similarity(a: &str, b: &str) -> f32 {
    let (x, y) = (shingles(a), shingles(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    let inter = x.intersection(&y).count() as f32;
    inter / x.len().min(y.len()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRONT: &str = include_str!("../tests/fixtures/web/news_front.html");
    const ARTICLE: &str = include_str!("../tests/fixtures/web/article.html");
    const SHELL: &str = include_str!("../tests/fixtures/web/jsshell.html");
    const RSS: &str = include_str!("../tests/fixtures/web/top.xml");
    const ATOM: &str = include_str!("../tests/fixtures/web/atom.xml");
    const PAGE2: &str = include_str!("../tests/fixtures/web/listing_page2.html");

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn an_article_is_read_without_the_clutter_around_it() {
        let p = analyze(ARTICLE, &u("https://news.example.com/2026/10/13/stocks-slide"));
        assert_eq!(p.kind, Kind::Article);
        assert_eq!(p.via, Via::Reader);
        assert_eq!(p.title, "Stocks slide as chipmakers drag Wall Street lower");
        assert_eq!(p.date, "2026-10-13 20:15");
        assert_eq!(p.byline, "Jane Rivera");
        assert_eq!(p.site, "Example Markets");
        let t = &p.text;
        assert!(t.contains("# Stocks slide as chipmakers"), "{t}");
        assert!(t.contains("The S&P 500 fell for a third straight session"), "{t}");
        assert!(t.contains("# Bond yields in focus") && t.contains("5.21%"), "{t}");
        assert!(t.contains("- The Dow Jones Industrial Average fell 0.4% to 51,102."), "list items are kept: {t}");
        for junk in ["Advertisement", "newsletter", "Related", "Older story", "Join the conversation", "Share on X", "Privacy policy", "Copyright", "Comments", "Subscribe"] {
            assert!(!t.contains(junk), "`{junk}` is clutter: {t}");
        }
        assert!(p.paragraphs >= 4);
        // the 4 paragraphs and the heading come out in reading order
        assert!(t.find("S&P 500 fell").unwrap() < t.find("Bond yields").unwrap() && t.find("Bond yields").unwrap() < t.find("Nasdaq Composite lost").unwrap());
    }

    #[test]
    fn a_front_page_is_a_listing_whose_story_links_rank_first() {
        let base = u("https://news.example.com/business");
        let p = analyze(FRONT, &base);
        assert_eq!(p.kind, Kind::Listing, "{:?}", p.kind);
        let stories = p.stories();
        assert_eq!(stories.len(), 9, "{:?}", stories.iter().map(|l| &l.text).collect::<Vec<_>>());
        assert!(stories[0].url.path.starts_with("/2026/10/13/"), "{:?}", stories[0]);
        let texts: Vec<&str> = p.links.iter().map(|l| l.text.as_str()).collect();
        for nav in ["Home", "Sign in", "Subscribe", "About us", "Privacy policy", "Contact us"] {
            assert!(!texts.contains(&nav), "menu entry `{nav}` is not a story: {texts:?}");
        }
        for nav in ["Markets", "World"] {
            let l = p.links.iter().find(|l| l.text == nav).expect("kept, but at the bottom");
            assert!(l.score < 0 && p.links.iter().position(|x| x.text == nav).unwrap() >= 9, "{l:?}");
        }
        assert!(!texts.iter().any(|t| t.contains("Facebook") || t.contains("Tweet")), "social buttons are dropped: {texts:?}");
        let ad = p.links.iter().find(|l| l.url.host == "ads.example.net").expect("kept but low");
        assert!(ad.score < ARTICLE_SCORE, "the ad is not a story: {ad:?}");
        assert!(p.links.iter().take(9).all(|l| l.url.host == "news.example.com"));
        assert_eq!(p.feeds, vec![u("https://news.example.com/feeds/top.xml")]);
        assert!(p.date.is_empty(), "a front page has no date of its own");
    }

    #[test]
    fn pagination_finds_next_and_numbered_pages() {
        let p = analyze(FRONT, &u("https://news.example.com/business"));
        assert_eq!(p.pager.next[0].1, u("https://news.example.com/business?page=2"));
        assert_eq!(p.pager.numbered.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec![2, 3], "the page itself is not listed");
        // page 2 of an archive: "older posts" is the next page, "previous" is not
        let p2 = analyze(PAGE2, &u("https://news.example.com/archive?page=2"));
        assert_eq!(p2.pager.next[0].1, u("https://news.example.com/archive?page=3"), "{:?}", p2.pager);
        assert!(p2.pager.next.iter().all(|(l, _)| !l.to_lowercase().contains("prev")));
        assert_eq!(p2.pager.numbered.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec![1, 3, 4]);
        // no links at all: the number in the address gives the next page
        let bare = analyze("<html><body><h1>Results</h1><p>Some results here and there, a few lines of text to read.</p></body></html>", &u("https://x.example/news/page/4/"));
        assert_eq!(bare.pager.next[0].1, u("https://x.example/news/page/5/"), "{:?}", bare.pager);
        assert_eq!(with_page(&u("https://x.example/list?q=a&page=2"), 7).unwrap().to_string(), "https://x.example/list?q=a&page=7");
        assert!(with_page(&u("https://x.example/list"), 2).is_none());
    }

    #[test]
    fn a_javascript_shell_still_gives_its_json_ld_and_open_graph_text() {
        let p = analyze(SHELL, &u("https://shell.example.com/a/1"));
        assert_eq!(p.via, Via::JsonLd);
        assert_eq!(p.kind, Kind::Article);
        assert_eq!(p.title, "Rupee hits record low as oil climbs");
        assert!(p.text.starts_with("The rupee weakened past 97 per dollar"), "{}", p.text);
        assert_eq!(p.date, "2026-10-13 09:30");
        assert_eq!(p.byline, "A. Kumar");
        assert!(!p.text.contains("enable JavaScript"));
        // only open-graph tags: the summary is all there is
        let og = analyze(r#"<html><head><title>x</title><meta property="og:title" content="Big news"><meta property="og:description" content="A short summary of the big news."></head><body><div id=app></div></body></html>"#, &u("https://shell.example.com/b"));
        assert_eq!((og.via, og.kind), (Via::Meta, Kind::Shell));
        assert_eq!((og.title.as_str(), og.text.as_str()), ("Big news", "A short summary of the big news."));
        // nothing at all
        let none = analyze("<html><body><div id=app></div><script>boot()</script></body></html>", &u("https://shell.example.com/c"));
        assert_eq!((none.via, none.kind), (Via::Nothing, Kind::Shell));
    }

    #[test]
    fn rss_and_atom_feeds_are_parsed() {
        let f = parse_feed(RSS, &u("https://news.example.com/feeds/top.xml")).unwrap();
        assert_eq!(f.title, "Example Markets - Top stories");
        assert_eq!(f.items.len(), 3);
        assert_eq!(f.items[0].title, "Stocks slide as chipmakers drag Wall Street lower");
        assert_eq!(f.items[0].link, "https://news.example.com/2026/10/13/stocks-slide-as-chipmakers-drag-wall-street-lower");
        assert_eq!(f.items[0].date, "2026-10-13 20:15");
        assert_eq!(f.items[0].summary, "The S&P 500 fell for a third day as a selloff in semiconductor shares outweighed bank earnings.", "tags are stripped from summaries");
        assert_eq!(f.items[1].title, "Oil prices edge higher & supply worries return");
        assert_eq!(f.items[2].link, "https://news.example.com/2026/10/11/gold-holds-near-record-high", "guid is the fallback link");
        assert_eq!(f.items[2].date, "2026-10-11 08:00");
        let a = parse_feed(ATOM, &u("https://blog.example.com/feed")).unwrap();
        assert_eq!(a.title, "Example Atom");
        assert_eq!(a.items[0].link, "https://blog.example.com/posts/first", "the alternate link, not rel=self");
        assert_eq!(a.items[1].link, "https://blog.example.com/posts/second");
        assert_eq!((a.items[0].date.as_str(), a.items[1].date.as_str()), ("2026-10-12 10:00", "2026-10-13 07:30"));
        assert_eq!(a.items[1].summary, "Second post body");
        assert!(parse_feed("<html><body>hi</body></html>", &u("https://x.example/")).is_none());
    }

    #[test]
    fn dates_words_and_similarity() {
        assert_eq!(norm_date("Tue, 13 Oct 2026 21:31:00 GMT"), "2026-10-13 21:31");
        assert_eq!(norm_date("2026-10-13T08:30:00+05:30"), "2026-10-13 08:30");
        assert_eq!(norm_date("2026-10-13"), "2026-10-13");
        assert_eq!(norm_date("sometime"), "sometime");
        assert_eq!(date_in_path("/business/2026/oct/13/story"), None);
        assert_eq!(date_in_path("/2026/10/13/story").as_deref(), Some("2026-10-13"));
        assert!(day_number("2026-10-13").unwrap() - day_number("2026-10-06").unwrap() == 7);
        assert_eq!(site_name("news.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(site_name("finance.yahoo.com"), "yahoo.com");
        assert!(same_site("www.cnbc.com", "cnbc.com") && same_site("news.example.com", "example.com") && !same_site("example.org", "example.com"));
        let a = "The S&P 500 fell for a third straight session on Tuesday as a selloff in semiconductor shares outweighed a strong start to the bank earnings season";
        let b = "Stocks: The S&P 500 fell for a third straight session on Tuesday as a selloff in semiconductor shares outweighed a strong start to the bank earnings season, Reuters said";
        assert!(similarity(a, b) > 0.8, "{}", similarity(a, b));
        assert!(similarity(a, "Heavy rain is expected across the coastal districts of Tamil Nadu over the next four days according to the weather office") < 0.1);
        assert_eq!(shorten("one two three four five six", 12), "one two…");
    }

    #[test]
    fn the_reason_a_page_cannot_be_read_is_said_plainly() {
        let m = status_reason("www.reuters.com", 403, "https://www.reuters.com/markets/");
        assert!(m.contains("403") && m.contains("blocks automated readers") && m.contains("try another source"), "{m}");
        assert!(status_reason("a.example", 404, "https://a.example/x").contains("no page at https://a.example/x"));
        assert!(status_reason("a.example", 429, "u").contains("rate-limiting"));
        assert!(status_reason("a.example", 402, "u").contains("paywall"));
        assert!(status_reason("a.example", 503, "u").contains("server error"));
        assert!(wall_reason("a.example", "Just a moment...", "Checking your browser", 20).unwrap().contains("bot check"));
        assert!(wall_reason("a.example", "", "Please enable JS and disable any ad blocker", 44).unwrap().contains("JavaScript"));
        assert!(wall_reason("a.example", "Before you continue", "We value your privacy", 40).unwrap().contains("consent"));
        assert!(wall_reason("consent.yahoo.com", "", "x", 1).unwrap().contains("consent"));
        assert_eq!(wall_reason("a.example", "A story", "a long article that mentions a captcha in passing", 5000), None, "a long page is never refused");
        assert!(net_reason("a.example", "io: timed out reading response").contains("did not answer in time"));
        assert!(net_reason("a.example", "dns error: failed to lookup address information").contains("could not be found"));
        assert_eq!(net_reason("a.example", "your account may not reach a.example"), "your account may not reach a.example");
    }

    #[test]
    fn known_feeds_prefer_the_section() {
        assert_eq!(known_feeds("www.bbc.com", "/news/business")[0], "https://feeds.bbci.co.uk/news/business/rss.xml");
        assert_eq!(known_feeds("www.thehindu.com", "/business/markets/")[0], "https://www.thehindu.com/business/markets/feeder/default.rss");
        assert!(known_feeds("unknown.example", "/").is_empty());
        assert!(known_feeds("www.marketwatch.com", "/")[0].contains("dowjones"));
    }

    #[test]
    fn excerpts_come_from_whole_paragraphs_and_end_on_a_sentence() {
        let p = analyze(ARTICLE, &u("https://news.example.com/a"));
        let e = excerpt(&p.text, 300);
        assert!(e.chars().count() <= 300 && e.starts_with("The S&P 500 fell"), "{e}");
        assert!(e.ends_with('.'), "{e}");
        assert!(!e.contains("# "));
    }
}
