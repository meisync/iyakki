//! `iyakki.markdown.render`: an analysis of Markdown text, no drawing. It answers how many lines the text takes at a width, the text without
//! its marks, and the links in it. The viewer (a shell component) draws; this is only for an app that wants to know before it asks.
//!
//! In-house and small on purpose: headings, lists, quotes, fenced and inline code, emphasis, links and images. It never runs or fetches
//! anything. A link whose scheme could run code (`javascript:`, `data:`, `vbscript:`) is not listed; it is counted in `blocked_links`.

use serde_json::{json, Value};

/// Longest text one call may analyse, in bytes.
pub const MAX_TEXT: usize = 1 << 20;
/// Links returned.
pub const MAX_LINKS: usize = 500;
/// Plain text returned, in characters.
pub const MAX_PLAIN: usize = 256 * 1024;
/// Pixels per character the line estimate assumes.
pub const PX_PER_CHAR: u32 = 8;
pub const DEFAULT_WIDTH: u32 = 640;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analysis {
    pub lines: usize,
    pub plain: String,
    pub truncated: bool,
    pub links: Vec<(String, String)>,
    pub blocked_links: usize,
    pub images: usize,
}

impl Analysis {
    pub fn to_json(&self) -> Value {
        json!({
            "lines": self.lines,
            "plain": self.plain,
            "truncated": self.truncated,
            "links": self.links.iter().map(|(t, u)| json!({"text": t, "url": u})).collect::<Vec<_>>(),
            "blockedLinks": self.blocked_links,
            "images": self.images,
        })
    }
}

fn scheme_blocked(url: &str) -> bool {
    let lower: String = url.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect::<String>().to_ascii_lowercase();
    ["javascript:", "data:", "vbscript:", "file:"].iter().any(|s| lower.starts_with(s))
}

/// Finds the next `needle` at or after a position, remembering the answer: the positions asked for only ever grow, so a long line of
/// brackets with no partner costs one pass, not one pass per bracket (the fuzz test found a megabyte of `[` taking minutes).
struct Next {
    needle: char,
    found: Option<usize>,
    none_from: Option<usize>,
}

impl Next {
    fn new(needle: char) -> Next {
        Next { needle, found: None, none_from: None }
    }
    fn at(&mut self, c: &[char], from: usize) -> Option<usize> {
        if self.none_from.is_some_and(|n| from >= n) {
            return None;
        }
        if let Some(f) = self.found.filter(|f| *f >= from) {
            return Some(f);
        }
        match c.get(from..).and_then(|rest| rest.iter().position(|x| *x == self.needle)) {
            Some(p) => {
                self.found = Some(from + p);
                self.found
            }
            None => {
                self.none_from = Some(from);
                None
            }
        }
    }
}

/// Inline marks removed; links and images collected. Returns the text.
fn inline(s: &str, links: &mut Vec<(String, String)>, blocked: &mut usize, images: &mut usize) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let (mut ticks, mut closes, mut parens) = (Next::new('`'), Next::new(']'), Next::new(')'));
    while i < c.len() {
        match c[i] {
            '\\' if i + 1 < c.len() => {
                out.push(c[i + 1]);
                i += 2;
            }
            '`' => {
                let end = ticks.at(&c, i + 1).map(|p| p - (i + 1));
                match end {
                    Some(n) => {
                        out.extend(&c[i + 1..i + 1 + n]);
                        i += n + 2;
                    }
                    None => {
                        out.push('`');
                        i += 1;
                    }
                }
            }
            '!' | '[' => {
                let img = c[i] == '!';
                let open = if img { i + 1 } else { i };
                if c.get(open) == Some(&'[') {
                    if let Some(close) = closes.at(&c, open + 1) {
                        if c.get(close + 1) == Some(&'(') {
                            if let Some(end) = parens.at(&c, close + 2) {
                                let label: String = c[open + 1..close].iter().collect();
                                let url: String = c[close + 2..end].iter().collect::<String>().split_whitespace().next().unwrap_or("").to_string();
                                let label = inline(&label, &mut Vec::new(), &mut 0, &mut 0);
                                if img {
                                    *images += 1;
                                } else if url.is_empty() {
                                } else if scheme_blocked(&url) {
                                    *blocked += 1;
                                } else if links.len() < MAX_LINKS {
                                    links.push((label.clone(), url));
                                }
                                out.push_str(&label);
                                i = end + 1;
                                continue;
                            }
                        }
                    }
                }
                out.push(c[i]);
                i += 1;
            }
            '*' | '_' | '~' => {
                // emphasis marks are dropped when they come in a run at a word edge; a lone one inside a word (snake_case) stays
                let inside_word = c[i] == '_' && i > 0 && i + 1 < c.len() && c[i - 1].is_alphanumeric() && c[i + 1].is_alphanumeric();
                if !inside_word {
                    i += 1;
                } else {
                    out.push(c[i]);
                    i += 1;
                }
            }
            x => {
                out.push(x);
                i += 1;
            }
        }
    }
    out
}

/// Analyse `text` for a column `width_px` wide (0: [`DEFAULT_WIDTH`]).
pub fn analyze(text: &str, width_px: u32) -> Result<Analysis, String> {
    if text.len() > MAX_TEXT {
        return Err("the text is longer than 1 MB".into());
    }
    let width = if width_px == 0 { DEFAULT_WIDTH } else { width_px };
    let per_line = (width / PX_PER_CHAR).clamp(8, 400) as usize;
    let (mut lines, mut links, mut blocked, mut images) = (0usize, Vec::new(), 0usize, 0usize);
    let mut plain = String::new();
    let mut plain_chars = 0usize;
    let mut in_fence = false;
    for raw in text.lines() {
        let t = raw.trim_end();
        let trimmed = t.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        let body: String = if in_fence {
            t.to_string()
        } else {
            let mut s = trimmed;
            // heading, quote, list marker
            s = s.trim_start_matches('#').trim_start();
            while let Some(r) = s.strip_prefix('>') {
                s = r.trim_start();
            }
            if let Some(r) = s.strip_prefix("- ").or_else(|| s.strip_prefix("* ")).or_else(|| s.strip_prefix("+ ")) {
                s = r;
            } else if let Some(p) = s.find(". ").filter(|p| *p > 0 && *p <= 3 && s[..*p].chars().all(|c| c.is_ascii_digit())) {
                s = &s[p + 2..];
            }
            if s.chars().all(|c| "-*_= ".contains(c)) && s.chars().filter(|c| "-*_=".contains(*c)).count() >= 3 {
                s = "";
            }
            inline(s, &mut links, &mut blocked, &mut images)
        };
        let n = body.chars().count();
        lines += if n == 0 { 1 } else { n.div_ceil(per_line) };
        if plain_chars < MAX_PLAIN {
            plain.push_str(&body);
            plain.push('\n');
            plain_chars += n + 1;
        }
    }
    let truncated = plain_chars > MAX_PLAIN;
    if truncated {
        plain = plain.chars().take(MAX_PLAIN).collect();
    }
    while plain.ends_with("\n\n") {
        plain.pop();
    }
    Ok(Analysis { lines, plain: plain.trim_end().to_string(), truncated, links, blocked_links: blocked, images })
}
