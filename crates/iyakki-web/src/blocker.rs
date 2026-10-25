//! Content blocker: parses the common filter-list syntax (EasyList / uBlock style network rules and simple element-hiding
//! rules) and answers "should this request be blocked?" in microseconds.
//!
//! Written from the public description of the syntax; no third-party code and no regular-expression engine (rules that are
//! regular expressions are skipped, so no input can make matching slow). The lists themselves are third-party data and are
//! not part of this program: the user downloads them (see [`crate::blocker_lists`]).
//!
//! Matching is by index, not by trying every rule: a rule that starts `||host.tld^` is found through its host (the request's
//! host and every parent domain are looked up), every other rule through the longest whole word it contains (the URL is cut
//! into words and each is looked up), and the few rules with no usable word are tried on every request.
//!
//! Supported: `||domain^`, `|http`, trailing `|`, `*`, `^`, `@@` exceptions, `$` options `script image stylesheet font media
//! xmlhttprequest subdocument ping websocket other document` (and `~` negations), `third-party`/`first-party` (`~third-party`),
//! `domain=a.com|~b.com`, `important`, `match-case` (ignored: matching is always case-insensitive), `@@...$document`
//! (the page is never filtered). Skipped and counted: regular-expression rules, `redirect`, `csp`, `removeparam`, `badfilter`,
//! `popup`, scriptlet and procedural cosmetic rules and every other option this program does not understand.

use std::collections::{BTreeSet, HashMap};

/// Most network rules kept (more are dropped and counted).
pub const MAX_RULES: usize = 300_000;
/// Most element-hiding rules kept.
pub const MAX_COSMETIC: usize = 40_000;
/// Longest filter line looked at; longer lines are skipped.
pub const MAX_LINE: usize = 2048;
/// Longest URL matched (the rest is ignored).
pub const MAX_URL: usize = 2048;
/// Most words of a URL looked up.
const MAX_WORDS: usize = 256;
/// Rules with no usable word are tried on every request: at most this many are kept.
const MAX_FALLBACK: usize = 4000;

/// What a request is for, as far as the rules care.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResType {
    Document,
    Subdocument,
    Stylesheet,
    Script,
    Image,
    Font,
    Media,
    Xhr,
    Ping,
    Websocket,
    Other,
}

const T_DOC: u16 = 1;
const T_SUB: u16 = 1 << 1;
const T_CSS: u16 = 1 << 2;
const T_SCRIPT: u16 = 1 << 3;
const T_IMAGE: u16 = 1 << 4;
const T_FONT: u16 = 1 << 5;
const T_MEDIA: u16 = 1 << 6;
const T_XHR: u16 = 1 << 7;
const T_PING: u16 = 1 << 8;
const T_WS: u16 = 1 << 9;
const T_OTHER: u16 = 1 << 10;
/// Every type except the page itself: what a rule without type options applies to.
const T_ALL_BUT_DOC: u16 = T_SUB | T_CSS | T_SCRIPT | T_IMAGE | T_FONT | T_MEDIA | T_XHR | T_PING | T_WS | T_OTHER;

impl ResType {
    pub fn bit(self) -> u16 {
        match self {
            ResType::Document => T_DOC,
            ResType::Subdocument => T_SUB,
            ResType::Stylesheet => T_CSS,
            ResType::Script => T_SCRIPT,
            ResType::Image => T_IMAGE,
            ResType::Font => T_FONT,
            ResType::Media => T_MEDIA,
            ResType::Xhr => T_XHR,
            ResType::Ping => T_PING,
            ResType::Websocket => T_WS,
            ResType::Other => T_OTHER,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ResType::Document => "page",
            ResType::Subdocument => "frame",
            ResType::Stylesheet => "style",
            ResType::Script => "script",
            ResType::Image => "image",
            ResType::Font => "font",
            ResType::Media => "media",
            ResType::Xhr => "request",
            ResType::Ping => "ping",
            ResType::Websocket => "websocket",
            ResType::Other => "other",
        }
    }

    /// A guess from the file name in the address, for transports that do not say what a request is for.
    pub fn guess(url: &str) -> ResType {
        let path = url.split(['?', '#']).next().unwrap_or("");
        let name = path.rsplit('/').next().unwrap_or("");
        let ext = match name.rsplit_once('.') {
            Some((_, e)) if e.len() <= 5 => e.to_ascii_lowercase(),
            _ => return ResType::Other,
        };
        match ext.as_str() {
            "js" | "mjs" => ResType::Script,
            "css" => ResType::Stylesheet,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "avif" | "bmp" => ResType::Image,
            "woff" | "woff2" | "ttf" | "otf" | "eot" => ResType::Font,
            "mp4" | "webm" | "mp3" | "ogg" | "m4a" | "m4s" | "ts" | "m3u8" | "mpd" | "wav" | "flac" => ResType::Media,
            "html" | "htm" => ResType::Subdocument,
            _ => ResType::Other,
        }
    }
}

fn option_type(name: &str) -> Option<u16> {
    Some(match name {
        "script" => T_SCRIPT,
        "image" => T_IMAGE,
        "stylesheet" | "css" => T_CSS,
        "font" => T_FONT,
        "media" => T_MEDIA,
        "xmlhttprequest" | "xhr" => T_XHR,
        "subdocument" | "frame" => T_SUB,
        "ping" | "beacon" => T_PING,
        "websocket" => T_WS,
        "other" | "object" | "object-subrequest" => T_OTHER,
        "document" | "doc" => T_DOC,
        _ => return None,
    })
}

const F_LEFT_DOMAIN: u8 = 1;
const F_LEFT: u8 = 2;
const F_RIGHT: u8 = 4;
const F_EXCEPTION: u8 = 8;
const F_IMPORTANT: u8 = 16;

/// `domain=` of a rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DomainSet {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl DomainSet {
    /// Does a page on `host` pass? An excluded domain always wins; with an include list the host must be under one of them.
    pub fn allows(&self, host: &str) -> bool {
        if host.is_empty() {
            return self.include.is_empty() && self.exclude.is_empty();
        }
        if self.exclude.iter().any(|d| under(host, d)) {
            return false;
        }
        self.include.is_empty() || self.include.iter().any(|d| under(host, d))
    }
}

/// `host` is `domain` or one of its subdomains.
pub fn under(host: &str, domain: &str) -> bool {
    host == domain || (host.len() > domain.len() && host.ends_with(domain) && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
}

/// One parsed network rule (before it is stored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetRule {
    /// The pattern, lower case, without the anchors and `@@`.
    pub pattern: String,
    pub left_domain: bool,
    pub left: bool,
    pub right: bool,
    pub exception: bool,
    pub important: bool,
    /// 0 for the default (every type but the page itself).
    pub types: u16,
    /// 0 any, 1 third party only, 2 first party only.
    pub party: u8,
    pub domains: Option<DomainSet>,
}

/// An element-hiding rule: hide what `selector` matches, on the listed domains (none: everywhere).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cosmetic {
    pub selector: String,
    pub domains: Option<DomainSet>,
    pub exception: bool,
}

/// What one line of a list is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// Empty, a comment or the list header.
    Nothing,
    Net(NetRule),
    Cosmetic(Cosmetic),
    /// A page the filters never touch (`@@||site^$document`).
    PageAllow(String),
    /// Understood but not supported; the reason is short and for the debug view.
    Skipped(&'static str),
}

/// The rule options as written after `$`.
struct Opts {
    types: u16,
    neg_types: u16,
    party: u8,
    domains: Option<DomainSet>,
    important: bool,
    document: bool,
}

fn parse_domain_list(v: &str) -> Option<DomainSet> {
    let mut set = DomainSet::default();
    for d in v.split('|') {
        let (neg, d) = match d.strip_prefix('~') {
            Some(r) => (true, r),
            None => (false, d),
        };
        let d = d.trim().to_ascii_lowercase();
        if d.is_empty() || d.len() > 253 || d.contains('/') || d.contains('*') || d.starts_with("regexp:") || !d.is_ascii() {
            return None;
        }
        if neg {
            set.exclude.push(d);
        } else {
            set.include.push(d);
        }
    }
    if set.include.is_empty() && set.exclude.is_empty() {
        None
    } else {
        Some(set)
    }
}

fn parse_opts(s: &str) -> Result<Opts, &'static str> {
    let mut o = Opts { types: 0, neg_types: 0, party: 0, domains: None, important: false, document: false };
    for raw in s.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let (neg, body) = match raw.strip_prefix('~') {
            Some(r) => (true, r),
            None => (false, raw),
        };
        let (name, value) = match body.split_once('=') {
            Some((n, v)) => (n.to_ascii_lowercase(), Some(v)),
            None => (body.to_ascii_lowercase(), None),
        };
        match (name.as_str(), value) {
            ("third-party" | "3p", None) => o.party = if neg { 2 } else { 1 },
            ("first-party" | "1p", None) => o.party = if neg { 1 } else { 2 },
            ("important", None) => o.important = true,
            ("match-case", None) => {}
            ("domain", Some(v)) if !neg => match parse_domain_list(v) {
                Some(d) => o.domains = Some(d),
                None => return Err("domain option not supported"),
            },
            ("all", None) => o.types |= T_ALL_BUT_DOC,
            (n, None) => match option_type(n) {
                Some(bit) => {
                    if bit == T_DOC {
                        o.document = !neg;
                    }
                    if neg {
                        o.neg_types |= bit;
                    } else {
                        o.types |= bit;
                    }
                }
                None => return Err("option not supported"),
            },
            _ => return Err("option not supported"),
        }
    }
    Ok(o)
}

fn is_regex_body(p: &str) -> bool {
    p.len() > 2 && p.starts_with('/') && p.ends_with('/') && p[1..p.len() - 1].chars().any(|c| matches!(c, '\\' | '(' | ')' | '[' | ']' | '{' | '}' | '+' | '?' | '^' | '$' | '|'))
}

/// A selector the blocker accepts: compound selectors of tags, `#id`, `.class` and simple `[attr]`/`[attr=value]` tests joined
/// by spaces and `>`, `+`, `~`. No pseudo-classes (they are where engines differ and where one bad rule would make a whole rule
/// list fail to load), no commas, no escapes.
pub fn safe_selector(s: &str) -> bool {
    if s.is_empty() || s.len() > 400 || !s.is_ascii() {
        return false;
    }
    let b = s.as_bytes();
    let mut i = 0;
    let mut compound = false; // inside a compound selector with at least one simple selector
    while i < b.len() {
        let c = b[i];
        match c {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b'*' => {
                compound = true;
                i += 1;
            }
            b'#' | b'.' => {
                // must be followed by a name character
                if !b.get(i + 1).is_some_and(|n| n.is_ascii_alphanumeric() || matches!(n, b'_' | b'-')) {
                    return false;
                }
                compound = true;
                i += 1;
            }
            b'[' => {
                let Some(end) = s[i..].find(']') else { return false };
                let inner = &s[i + 1..i + end];
                if !safe_attr(inner) {
                    return false;
                }
                compound = true;
                i += end + 1;
            }
            b' ' | b'>' | b'+' | b'~' => {
                if !compound && c != b' ' {
                    return false;
                }
                // a combinator needs a compound on each side
                let mut j = i;
                while j < b.len() && matches!(b[j], b' ' | b'>' | b'+' | b'~') {
                    j += 1;
                }
                if j >= b.len() {
                    return false; // trailing combinator
                }
                if i == 0 {
                    return false; // leading combinator
                }
                compound = false;
                i = j;
            }
            _ => return false,
        }
    }
    compound
}

fn safe_attr(inner: &str) -> bool {
    let b = inner.as_bytes();
    let name_end = b.iter().position(|c| !(c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))).unwrap_or(b.len());
    if name_end == 0 {
        return false;
    }
    let rest = &inner[name_end..];
    if rest.is_empty() {
        return true;
    }
    let val = if let Some(v) = rest.strip_prefix('=') {
        v
    } else if rest.len() > 2 && matches!(rest.as_bytes()[0], b'^' | b'$' | b'*' | b'~' | b'|') && rest.as_bytes()[1] == b'=' {
        &rest[2..]
    } else {
        return false;
    };
    let v = val.trim_end_matches([' ', 'i', 's']);
    let v = if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) { &v[1..v.len() - 1] } else { v };
    !v.is_empty() && v.len() < 200 && v.bytes().all(|c| c.is_ascii_graphic() || c == b' ') && !v.contains(['"', '\'', '\\', ']', '{', '}', ';', '<', '>'])
}

/// Cosmetic marker positions: the first of `##`, `#@#`, `#?#`, `#$#`, `#%#`, `#@$#`, `#@?#`, `#@%#`.
fn cosmetic_split(line: &str) -> Option<(&str, &str, &str)> {
    for m in ["#@$#", "#@?#", "#@%#", "#@#", "#?#", "#$#", "#%#", "##"] {
        if let Some(p) = line.find(m) {
            let dom = &line[..p];
            if dom.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_' | b',' | b'~' | b'*' | b'|')) {
                return Some((dom, m, &line[p + m.len()..]));
            }
        }
    }
    None
}

/// Parse one line of a filter list. Never panics, whatever the line holds.
pub fn parse_line(line: &str) -> Line {
    let line = line.trim();
    if line.is_empty() || line.starts_with('!') || (line.starts_with('[') && line.ends_with(']')) {
        return Line::Nothing;
    }
    if line.len() > MAX_LINE {
        return Line::Skipped("line too long");
    }
    if let Some((dom, marker, sel)) = cosmetic_split(line) {
        if marker != "##" && marker != "#@#" {
            return Line::Skipped("procedural or scriptlet rule");
        }
        let sel = sel.trim();
        if !safe_selector(sel) {
            return Line::Skipped("selector not supported");
        }
        let domains = if dom.is_empty() {
            None
        } else {
            match parse_domain_list(&dom.replace(',', "|")) {
                Some(d) => Some(d),
                None => return Line::Skipped("cosmetic domain not supported"),
            }
        };
        return Line::Cosmetic(Cosmetic { selector: sel.to_string(), domains, exception: marker == "#@#" });
    }
    let (exception, mut body) = match line.strip_prefix("@@") {
        Some(r) => (true, r),
        None => (false, line),
    };
    // options start at the last `$` unless that dollar belongs to a regular expression
    let mut opts_text = "";
    if let Some(p) = body.rfind('$') {
        let after = &body[p + 1..];
        // options start with a name (`script`, `~third-party`, `domain=...`); a `$` inside a path is not an option
        let first: &str = after.split([',', '=']).next().unwrap_or("");
        let looks_like_opts = !first.is_empty() && first.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'~' | b'-' | b'_'));
        if looks_like_opts {
            opts_text = after;
            body = &body[..p];
        }
    }
    if is_regex_body(body) {
        return Line::Skipped("regular expression rule");
    }
    let opts = match parse_opts(opts_text) {
        Ok(o) => o,
        Err(why) => return Line::Skipped(why),
    };
    let mut types = opts.types;
    if types == 0 && opts.neg_types != 0 {
        types = T_ALL_BUT_DOC & !opts.neg_types;
        if types == 0 {
            return Line::Skipped("no type left");
        }
    } else if opts.neg_types != 0 {
        types &= !opts.neg_types;
        if types == 0 {
            return Line::Skipped("no type left");
        }
    }
    let (mut left_domain, mut left, mut right) = (false, false, false);
    let mut pat = body;
    if let Some(r) = pat.strip_prefix("||") {
        left_domain = true;
        pat = r;
    } else if let Some(r) = pat.strip_prefix('|') {
        left = true;
        pat = r;
    }
    if let Some(r) = pat.strip_suffix('|') {
        right = true;
        pat = r;
    }
    if exception && opts.document && opts.types == T_DOC {
        // `@@||site^$document`: the page is never filtered
        if left_domain {
            let host: String = pat.chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
            if host.contains('.') && pat[host.len()..].chars().all(|c| matches!(c, '^' | '/')) {
                return Line::PageAllow(host.to_ascii_lowercase());
            }
        }
        return Line::Skipped("document exception not supported");
    }
    if pat.is_empty() && !left_domain && !left {
        return Line::Skipped("empty pattern");
    }
    if !pat.bytes().any(|c| c != b'*' && c != b'^') {
        return Line::Skipped("pattern with no text");
    }
    if pat.chars().any(|c| c.is_control() || c == ' ') {
        return Line::Skipped("space or control character in pattern");
    }
    let mut pattern = pat.to_ascii_lowercase();
    // collapse runs of `*`
    while pattern.contains("**") {
        pattern = pattern.replace("**", "*");
    }
    Line::Net(NetRule { pattern, left_domain, left, right, exception, important: opts.important, types, party: opts.party, domains: opts.domains })
}

#[derive(Clone, Copy)]
struct Rule {
    off: u32,
    len: u16,
    flags: u8,
    party: u8,
    types: u16,
    /// 1 + index into `domains`, 0 for none.
    dom: u32,
    list: u8,
}

#[derive(Default)]
struct Index {
    hosts: HashMap<String, Vec<u32>>,
    words: HashMap<u64, Vec<u32>>,
    fallback: Vec<u32>,
}

/// What a compile kept and dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub blocking: usize,
    pub exceptions: usize,
    pub cosmetic: usize,
    pub page_allows: usize,
    /// Lines understood but not supported (regular expressions, redirects, ...).
    pub skipped: usize,
    /// Rules dropped because a limit was reached.
    pub dropped: usize,
    pub lists: usize,
    /// Lines that were neither a comment nor a rule the program could read.
    pub lines: usize,
}

/// Builds a [`Blocker`] from filter lists.
pub struct Compiler {
    arena: Vec<u8>,
    rules: Vec<Rule>,
    rule_lines: Vec<NetRule>,
    domains: Vec<DomainSet>,
    cosmetic: Vec<Cosmetic>,
    page_allows: BTreeSet<String>,
    stats: Stats,
    max_rules: usize,
}

impl Default for Compiler {
    fn default() -> Self {
        Compiler::with_limit(MAX_RULES)
    }
}

impl Compiler {
    pub fn new() -> Compiler {
        Compiler::default()
    }

    /// A compiler that keeps at most `max_rules` network rules.
    pub fn with_limit(max_rules: usize) -> Compiler {
        Compiler { arena: Vec::new(), rules: Vec::new(), rule_lines: Vec::new(), domains: Vec::new(), cosmetic: Vec::new(), page_allows: BTreeSet::new(), stats: Stats::default(), max_rules, }
    }

    /// Add one list (its whole text). `list` numbers the list for the debug view (0 to 255).
    pub fn add_list(&mut self, text: &str, list: u8) {
        self.stats.lists += 1;
        for line in text.lines() {
            match parse_line(line) {
                Line::Nothing => {}
                Line::Skipped(_) => {
                    self.stats.skipped += 1;
                    self.stats.lines += 1;
                }
                Line::PageAllow(h) => {
                    self.stats.lines += 1;
                    if self.page_allows.len() < 20_000 && self.page_allows.insert(h) {
                        self.stats.page_allows += 1;
                    }
                }
                Line::Cosmetic(c) => {
                    self.stats.lines += 1;
                    if self.cosmetic.len() >= MAX_COSMETIC {
                        self.stats.dropped += 1;
                    } else {
                        self.cosmetic.push(c);
                    }
                }
                Line::Net(r) => {
                    self.stats.lines += 1;
                    if self.rules.len() >= self.max_rules || self.arena.len() > (u32::MAX as usize) - 4096 {
                        self.stats.dropped += 1;
                        continue;
                    }
                    self.push(r, list);
                }
            }
        }
    }

    fn push(&mut self, r: NetRule, list: u8) {
        let off = self.arena.len() as u32;
        let bytes = r.pattern.as_bytes();
        let len = bytes.len().min(u16::MAX as usize);
        self.arena.extend_from_slice(&bytes[..len]);
        let mut flags = 0;
        if r.left_domain {
            flags |= F_LEFT_DOMAIN;
        }
        if r.left {
            flags |= F_LEFT;
        }
        if r.right {
            flags |= F_RIGHT;
        }
        if r.exception {
            flags |= F_EXCEPTION;
            self.stats.exceptions += 1;
        } else {
            self.stats.blocking += 1;
        }
        if r.important {
            flags |= F_IMPORTANT;
        }
        let dom = match &r.domains {
            Some(d) => {
                self.domains.push(d.clone());
                self.domains.len() as u32
            }
            None => 0,
        };
        self.rules.push(Rule { off, len: len as u16, flags, party: r.party, types: r.types, dom, list });
        self.rule_lines.push(r);
    }

    /// Build the matcher. Keeps the parsed rules (for the WebKit rule-list export) in the result.
    pub fn finish(self) -> Blocker {
        let mut block = Index::default();
        let mut exc = Index::default();
        let mut fallback_dropped = 0;
        for (i, r) in self.rules.iter().enumerate() {
            let idx = if r.flags & F_EXCEPTION != 0 { &mut exc } else { &mut block };
            let pat = &self.arena[r.off as usize..r.off as usize + r.len as usize];
            match index_key(pat, r.flags) {
                Key::Host(h) => idx.hosts.entry(h).or_default().push(i as u32),
                Key::Word(w) => idx.words.entry(w).or_default().push(i as u32),
                Key::None => {
                    if idx.fallback.len() < MAX_FALLBACK {
                        idx.fallback.push(i as u32);
                    } else {
                        fallback_dropped += 1;
                    }
                }
            }
        }
        let mut stats = self.stats;
        stats.dropped += fallback_dropped;
        let mut cos_generic = Vec::new();
        let mut cos_by_domain: Vec<Cosmetic> = Vec::new();
        let mut cos_exceptions: Vec<Cosmetic> = Vec::new();
        for c in self.cosmetic {
            if c.exception {
                cos_exceptions.push(c);
            } else if c.domains.is_none() {
                cos_generic.push(c.selector);
            } else {
                cos_by_domain.push(c);
            }
        }
        stats.cosmetic = cos_generic.len() + cos_by_domain.len();
        Blocker { arena: self.arena, rules: self.rules, rule_lines: self.rule_lines, domains: self.domains, block, exc, page_allows: self.page_allows, cos_generic, cos_by_domain, cos_exceptions, stats }
    }
}

enum Key {
    Host(String),
    Word(u64),
    None,
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn is_word(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit()
}

fn index_key(pat: &[u8], flags: u8) -> Key {
    if flags & F_LEFT_DOMAIN != 0 {
        let end = pat.iter().position(|&b| !(b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))).unwrap_or(pat.len());
        // the host part must be followed by something that ends a host: `^`, `/` or `:`
        if end > 0 && pat[..end].contains(&b'.') && end < pat.len() && matches!(pat[end], b'^' | b'/' | b':') && !pat[..end].ends_with(b".") {
            if let Ok(h) = std::str::from_utf8(&pat[..end]) {
                return Key::Host(h.to_string());
            }
        }
    }
    // longest whole word: bounded on both sides by something that is not a word character
    let mut best: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < pat.len() {
        if !is_word(pat[i]) {
            i += 1;
            continue;
        }
        let s = i;
        while i < pat.len() && is_word(pat[i]) {
            i += 1;
        }
        let e = i;
        let left_ok = if s == 0 { flags & (F_LEFT | F_LEFT_DOMAIN) != 0 } else { pat[s - 1] != b'*' };
        let right_ok = if e == pat.len() { flags & F_RIGHT != 0 } else { pat[e] != b'*' };
        if left_ok && right_ok && e - s >= 2 && best.is_none_or(|(bs, be)| e - s > be - bs) {
            best = Some((s, e));
        }
    }
    match best {
        Some((s, e)) => Key::Word(fnv(&pat[s..e])),
        None => Key::None,
    }
}

/// A request to check.
#[derive(Clone, Copy, Debug)]
pub struct ReqInfo<'a> {
    pub url: &'a str,
    /// The address (or just the host) of the page that asked; empty when unknown.
    pub initiator: &'a str,
    pub kind: ResType,
}

/// The answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Blocked by rule number `rule` (see [`Blocker::describe`]).
    Block { rule: u32 },
}

impl Verdict {
    pub fn is_block(&self) -> bool {
        matches!(self, Verdict::Block { .. })
    }
}

/// The compiled lists.
pub struct Blocker {
    arena: Vec<u8>,
    rules: Vec<Rule>,
    rule_lines: Vec<NetRule>,
    domains: Vec<DomainSet>,
    block: Index,
    exc: Index,
    page_allows: BTreeSet<String>,
    cos_generic: Vec<String>,
    cos_by_domain: Vec<Cosmetic>,
    cos_exceptions: Vec<Cosmetic>,
    stats: Stats,
}

impl Default for Blocker {
    fn default() -> Self {
        Compiler::new().finish()
    }
}

/// `host` of an address (or the text itself when it has no scheme), lower case, without user, port and trailing dot.
pub fn host_of(url: &str) -> String {
    let (a, _, _) = authority(url.as_bytes());
    let rest = &url[a.0..a.1];
    let rest = rest.rsplit('@').next().unwrap_or(rest);
    let rest = if rest.starts_with('[') { rest.split(']').next().unwrap_or(rest) } else { rest.split(':').next().unwrap_or(rest) };
    rest.trim_end_matches('.').to_ascii_lowercase()
}

/// (start, end) of the authority (host with port, after any user) of `url`, plus the host end (before any port).
fn authority(u: &[u8]) -> ((usize, usize), usize, bool) {
    let start = match find(u, b"://", 0) {
        Some(p) if p <= 12 => p + 3,
        _ => 0,
    };
    let mut end = start;
    while end < u.len() && !matches!(u[end], b'/' | b'?' | b'#') {
        end += 1;
    }
    // skip user info
    let mut s = start;
    if let Some(at) = u[start..end].iter().rposition(|&b| b == b'@') {
        s = start + at + 1;
    }
    let mut host_end = end;
    if let Some(c) = u[s..end].iter().rposition(|&b| b == b':') {
        if !u[s..end].contains(&b']') || u[s + c..end].iter().all(|b| *b != b']') {
            host_end = s + c;
        }
    }
    ((s, end), host_end, start > 0)
}

fn find(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    if n.is_empty() || h.len() < n.len() || from > h.len() - n.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| &h[i..i + n.len()] == n)
}

fn is_sep(b: u8) -> bool {
    !(b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'%'))
}

/// Match `seg` (literal bytes and `^`) at `pos`; the end of the match.
fn seg_at(seg: &[u8], url: &[u8], mut pos: usize) -> Option<usize> {
    for &b in seg {
        if b == b'^' {
            if pos == url.len() {
                continue;
            }
            if is_sep(url[pos]) {
                pos += 1;
            } else {
                return None;
            }
        } else if pos < url.len() && url[pos] == b {
            pos += 1;
        } else {
            return None;
        }
    }
    Some(pos)
}

/// Wildcard match of a rule's pattern against a lower-case URL. Linear in the URL length times the pattern length: with only
/// `*` as a wildcard, taking the earliest place for each piece is always enough, so nothing is tried twice.
fn glob(pat: &[u8], flags: u8, url: &[u8], auth: (usize, usize)) -> bool {
    let nseg = pat.iter().filter(|&&b| b == b'*').count() + 1;
    let mut cur = 0usize;
    for (i, seg) in pat.split(|&b| b == b'*').enumerate() {
        if seg.is_empty() {
            continue;
        }
        let first = i == 0;
        let last = i == nseg - 1;
        let right = last && flags & F_RIGHT != 0;
        if first && flags & F_LEFT != 0 {
            match seg_at(seg, url, 0) {
                Some(e) if !right || e == url.len() => cur = e,
                _ => return false,
            }
            continue;
        }
        if first && flags & F_LEFT_DOMAIN != 0 {
            let mut found = None;
            let mut p = auth.0;
            while p < auth.1 || p == auth.0 {
                if let Some(e) = seg_at(seg, url, p) {
                    if !right || e == url.len() {
                        found = Some(e);
                        break;
                    }
                }
                // next label boundary inside the authority
                match url[p..auth.1.min(url.len())].iter().position(|&b| b == b'.') {
                    Some(d) => p += d + 1,
                    None => break,
                }
            }
            match found {
                Some(e) => {
                    cur = e;
                    continue;
                }
                None => return false,
            }
        }
        // earliest place at or after `cur`
        let mut p = cur;
        let mut found = None;
        while p <= url.len() {
            if let Some(e) = seg_at(seg, url, p) {
                if !right || e == url.len() {
                    found = Some(e);
                    break;
                }
            }
            p += 1;
        }
        match found {
            Some(e) => cur = e,
            None => return false,
        }
    }
    true
}

/// A small table of public suffixes with two labels, enough for the sites that matter; everything else uses the last two labels.
const TWO_LABEL_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "ltd.uk", "plc.uk", "com.au", "net.au", "org.au", "edu.au", "gov.au", "co.nz", "org.nz", "net.nz", "co.jp", "or.jp", "ne.jp", "ac.jp", "co.in", "net.in", "org.in", "ac.in", "gov.in",
    "com.br", "net.br", "org.br", "com.cn", "net.cn", "org.cn", "gov.cn", "com.mx", "org.mx", "co.za", "org.za", "com.tr", "org.tr", "com.sg", "com.hk", "com.tw", "co.kr", "or.kr", "com.ar", "com.co", "co.id", "co.il", "com.my", "com.ph", "com.pk", "com.ua", "com.eg",
    "com.sa", "com.ng", "co.ke", "com.vn", "com.pe", "com.ve", "com.ec", "com.uy", "com.bd", "com.np", "com.lk", "github.io", "gitlab.io", "blogspot.com", "herokuapp.com", "appspot.com", "netlify.app", "vercel.app", "pages.dev", "web.app", "firebaseapp.com", "cloudfront.net", "azurewebsites.net",
];

/// The registrable domain of a host (the site, for the third-party test). An address with letters only in its last label and
/// digits elsewhere (an IP number) is returned whole.
pub fn site_of(host: &str) -> &str {
    if host.is_empty() || host.bytes().all(|b| b.is_ascii_digit() || b == b'.') || host.contains(':') {
        return host;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 {
        return host;
    }
    let two = labels[labels.len() - 2..].join(".");
    let keep = if TWO_LABEL_SUFFIXES.contains(&two.as_str()) { 3 } else { 2 };
    let skip: usize = labels[..labels.len() - keep.min(labels.len())].iter().map(|l| l.len() + 1).sum();
    &host[skip..]
}

impl Blocker {
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Network rules kept (blocking and exceptions).
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Roughly how much memory the compiled lists use (rule text, rule records, index tables, hiding selectors).
    pub fn memory_estimate(&self) -> usize {
        let idx = |i: &Index| i.hosts.iter().map(|(k, v)| 48 + k.len() + v.len() * 4).sum::<usize>() + i.words.values().map(|v| 56 + v.len() * 4).sum::<usize>() + i.fallback.len() * 4;
        let parsed: usize = self.rule_lines.iter().map(|r| 96 + r.pattern.len()).sum();
        self.arena.len() + self.rules.len() * std::mem::size_of::<Rule>() + idx(&self.block) + idx(&self.exc) + parsed + self.cos_generic.iter().map(|s| 24 + s.len()).sum::<usize>() + self.cos_by_domain.len() * 120
    }

    /// The parsed network rules, for exports.
    pub fn net_rules(&self) -> &[NetRule] {
        &self.rule_lines
    }

    pub fn generic_selectors(&self) -> &[String] {
        &self.cos_generic
    }

    pub fn domain_cosmetics(&self) -> &[Cosmetic] {
        &self.cos_by_domain
    }

    pub fn cosmetic_exceptions(&self) -> &[Cosmetic] {
        &self.cos_exceptions
    }

    /// Hosts whose pages are never filtered because a list says so.
    pub fn page_allows(&self) -> &BTreeSet<String> {
        &self.page_allows
    }

    /// Hiding selectors for a page on `host`: the generic ones and those listed for the host, minus the exceptions for it.
    pub fn hide_selectors(&self, host: &str) -> Vec<String> {
        let host = host.to_ascii_lowercase();
        if self.page_allowed(&host) {
            return Vec::new();
        }
        let off = |sel: &str| self.cos_exceptions.iter().any(|e| e.selector == sel && e.domains.as_ref().is_none_or(|d| d.allows(&host)));
        let mut v: Vec<String> = self.cos_generic.iter().filter(|s| !off(s)).cloned().collect();
        v.extend(self.cos_by_domain.iter().filter(|c| c.domains.as_ref().is_some_and(|d| d.allows(&host)) && !off(&c.selector)).map(|c| c.selector.clone()));
        v
    }

    fn page_allowed(&self, host: &str) -> bool {
        if self.page_allows.is_empty() {
            return false;
        }
        let mut h = host;
        loop {
            if self.page_allows.contains(h) {
                return true;
            }
            match h.split_once('.') {
                Some((_, rest)) if rest.contains('.') => h = rest,
                _ => return false,
            }
        }
    }

    /// A line for the debug view: the rule as it would be written in a list.
    pub fn describe(&self, rule: u32) -> String {
        let Some(r) = self.rule_lines.get(rule as usize) else { return String::new() };
        let mut s = String::new();
        if r.exception {
            s.push_str("@@");
        }
        if r.left_domain {
            s.push_str("||");
        } else if r.left {
            s.push('|');
        }
        s.push_str(&r.pattern);
        if r.right {
            s.push('|');
        }
        let mut opts: Vec<String> = Vec::new();
        for (name, bit) in [("document", T_DOC), ("subdocument", T_SUB), ("stylesheet", T_CSS), ("script", T_SCRIPT), ("image", T_IMAGE), ("font", T_FONT), ("media", T_MEDIA), ("xmlhttprequest", T_XHR), ("ping", T_PING), ("websocket", T_WS), ("other", T_OTHER)] {
            if r.types & bit != 0 && r.types != T_ALL_BUT_DOC {
                opts.push(name.into());
            }
        }
        match r.party {
            1 => opts.push("third-party".into()),
            2 => opts.push("~third-party".into()),
            _ => {}
        }
        if let Some(d) = &r.domains {
            let mut l: Vec<String> = d.include.clone();
            l.extend(d.exclude.iter().map(|x| format!("~{x}")));
            opts.push(format!("domain={}", l.join("|")));
        }
        if r.important {
            opts.push("important".into());
        }
        if !opts.is_empty() {
            s.push('$');
            s.push_str(&opts.join(","));
        }
        s
    }

    /// Which list (as numbered in [`Compiler::add_list`]) a rule came from.
    pub fn list_of(&self, rule: u32) -> Option<u8> {
        self.rules.get(rule as usize).map(|r| r.list)
    }

    /// Should this request be blocked? Allocation-light: one lower-cased copy of the URL, then index lookups.
    pub fn check(&self, req: &ReqInfo) -> Verdict {
        if self.rules.is_empty() {
            return Verdict::Allow;
        }
        let mut url_s = req.url.split('#').next().unwrap_or("").to_ascii_lowercase();
        if url_s.len() > MAX_URL {
            let mut cut = MAX_URL;
            while !url_s.is_char_boundary(cut) {
                cut -= 1;
            }
            url_s.truncate(cut);
        }
        let url = url_s.as_bytes();
        let (auth, host_end, has_scheme) = authority(url);
        if !has_scheme {
            return Verdict::Allow;
        }
        let host = &url_s[auth.0..host_end];
        let initiator_host = if req.initiator.is_empty() { String::new() } else { host_of(req.initiator) };
        if !self.page_allows.is_empty() && (self.page_allowed(&initiator_host) || (req.kind == ResType::Document && self.page_allowed(host))) {
            return Verdict::Allow;
        }
        let third = !initiator_host.is_empty() && site_of(host) != site_of(&initiator_host);
        let ctx = Ctx { url, auth, host: &url_s[auth.0..host_end], initiator: &initiator_host, third, known_initiator: !initiator_host.is_empty(), kind: req.kind.bit() };
        let Some(hit) = self.first_match(&self.block, &ctx, false) else { return Verdict::Allow };
        if self.rules[hit as usize].flags & F_IMPORTANT != 0 {
            return Verdict::Block { rule: hit };
        }
        if self.exc.hosts.is_empty() && self.exc.words.is_empty() && self.exc.fallback.is_empty() {
            return Verdict::Block { rule: hit };
        }
        if self.first_match(&self.exc, &ctx, true).is_some() {
            Verdict::Allow
        } else {
            Verdict::Block { rule: hit }
        }
    }

    /// Convenience: is this request blocked?
    pub fn is_blocked(&self, url: &str, initiator: &str, kind: ResType) -> bool {
        self.check(&ReqInfo { url, initiator, kind }).is_block()
    }

    fn first_match(&self, idx: &Index, c: &Ctx, _exceptions: bool) -> Option<u32> {
        // host index: the host and every parent domain
        if !idx.hosts.is_empty() {
            let mut h = c.host;
            loop {
                if let Some(list) = idx.hosts.get(h) {
                    for &i in list {
                        if self.rule_matches(i, c) {
                            return Some(i);
                        }
                    }
                }
                match h.split_once('.') {
                    Some((_, rest)) if !rest.is_empty() => h = rest,
                    _ => break,
                }
            }
        }
        if !idx.words.is_empty() {
            let mut n = 0;
            let u = c.url;
            let mut i = 0;
            while i < u.len() && n < MAX_WORDS {
                if !is_word(u[i]) {
                    i += 1;
                    continue;
                }
                let s = i;
                while i < u.len() && is_word(u[i]) {
                    i += 1;
                }
                n += 1;
                if i - s < 2 {
                    continue;
                }
                if let Some(list) = idx.words.get(&fnv(&u[s..i])) {
                    for &r in list {
                        if self.rule_matches(r, c) {
                            return Some(r);
                        }
                    }
                }
            }
        }
        idx.fallback.iter().copied().find(|&r| self.rule_matches(r, c))
    }

    fn rule_matches(&self, i: u32, c: &Ctx) -> bool {
        let r = &self.rules[i as usize];
        let types = if r.types == 0 { T_ALL_BUT_DOC } else { r.types };
        if types & c.kind == 0 {
            return false;
        }
        match r.party {
            1 if !(c.known_initiator && c.third) => return false,
            2 if !(c.known_initiator && !c.third) => return false,
            _ => {}
        }
        if r.dom != 0 {
            if !c.known_initiator {
                return false;
            }
            let d = &self.domains[r.dom as usize - 1];
            if !d.allows(c.initiator) {
                return false;
            }
        }
        glob(&self.arena[r.off as usize..r.off as usize + r.len as usize], r.flags, c.url, c.auth)
    }
}

struct Ctx<'a> {
    url: &'a [u8],
    auth: (usize, usize),
    host: &'a str,
    initiator: &'a str,
    third: bool,
    known_initiator: bool,
    kind: u16,
}

/// Compile several lists (text, in order) with the default limits.
pub fn compile_lists<'a>(lists: impl IntoIterator<Item = &'a str>) -> Blocker {
    let mut c = Compiler::new();
    for (i, l) in lists.into_iter().enumerate() {
        c.add_list(l, i.min(255) as u8);
    }
    c.finish()
}

#[cfg(test)]
#[path = "blocker_tests.rs"]
mod tests;
