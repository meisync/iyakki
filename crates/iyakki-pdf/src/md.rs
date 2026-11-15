//! Markdown-ish text (what a model or a tile writes) as blocks for the PDF writer: headings (`#` to `######`), paragraphs, list items
//! (`-`, `*`, `+`, `1.`; two spaces of indent per level), quotes (`>`), fenced code (```), rules (`---`), tables written with `|` (kept
//! as aligned monospace text) and pictures on a line of their own (`![alt](address)`), which the caller resolves.
//!
//! The text is data: nothing in it is ever run or followed. Inline marks (`**bold**`, `*italic*`, `` `code` ``) are dropped, a link
//! keeps its words and its address. Text outside Windows-1252 is not written by the writer (it would become `?`), so
//! [`outside_latin`] finds it and the caller refuses; [`fold`] first turns a few common look-alikes (arrows, the minus sign) into
//! plain characters.

use crate::write::{winansi, Block, PdfImage};

/// At most this many blocks come out of one text.
pub const MAX_BLOCKS: usize = 20_000;
/// At most this many pictures are asked for.
pub const MAX_PICTURES: usize = 40;

/// A few look-alikes the standard fonts lack, as plain characters.
pub fn fold(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{2011}' | '\u{2010}' | '\u{2212}' => o.push('-'),
            '\u{2192}' => o.push_str("->"),
            '\u{2190}' => o.push_str("<-"),
            '\u{2264}' => o.push_str("<="),
            '\u{2265}' => o.push_str(">="),
            '\u{2713}' | '\u{2714}' => o.push_str("(yes)"),
            '\u{2717}' | '\u{2718}' => o.push_str("(no)"),
            '\u{200b}' | '\u{feff}' | '\u{fe0f}' => {}
            '\u{2009}' | '\u{202f}' | '\u{2002}' | '\u{2003}' => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

/// How many characters in the title and blocks the standard fonts cannot write, and the first few distinct ones (at most `max`).
pub fn outside_latin(title: &str, blocks: &[Block], max: usize) -> (usize, Vec<char>) {
    let mut count = 0usize;
    let mut seen: Vec<char> = Vec::new();
    let mut scan = |s: &str| {
        for c in s.chars() {
            if c.is_whitespace() || winansi(c).is_some() || crate::embed::supports(c) {
                continue;
            }
            count += 1;
            if seen.len() < max && !seen.contains(&c) {
                seen.push(c);
            }
        }
    };
    scan(title);
    for b in blocks {
        match b {
            Block::Heading(_, t) | Block::Para(t) | Block::Quote(t) | Block::Code(t) => scan(t),
            Block::Bullet(_, m, t) => {
                scan(m);
                scan(t);
            }
            Block::Rule | Block::Image(_) | Block::PageBreak => {}
        }
    }
    (count, seen)
}

/// How far ahead a mark looks for its closing mark. A line of nothing but opening marks would otherwise cost the square of its length
/// (300 000 characters of `[` took minutes); a link or an emphasis longer than this is written as it is, marks included.
const LOOK_AHEAD: usize = 4096;

/// Drop inline marks: `**x**`, `__x__`, `*x*`, `_x_`, `` `x` ``; `[words](address)` becomes `words (address)`.
pub fn strip_inline(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut o = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == '`' {
            i += 1;
            continue;
        }
        if (ch == '*' || ch == '_') && i + 1 < c.len() && c[i + 1] == ch {
            i += 2;
            continue;
        }
        if (ch == '*' || ch == '_') && i + 1 < c.len() && !c[i + 1].is_whitespace() && (i == 0 || !c[i - 1].is_alphanumeric()) {
            // a single mark opens emphasis only when it closes later on the line
            if let Some(j) = (i + 2..c.len().min(i + 2 + LOOK_AHEAD)).find(|&j| c[j] == ch && !c[j - 1].is_whitespace() && c.get(j + 1).is_none_or(|n| !n.is_alphanumeric())) {
                o.extend(&c[i + 1..j]);
                i = j + 1;
                continue;
            }
        }
        if ch == '[' {
            if let Some(close) = (i + 1..c.len().min(i + 1 + LOOK_AHEAD)).find(|&j| c[j] == ']') {
                if c.get(close + 1) == Some(&'(') {
                    if let Some(end) = (close + 2..c.len().min(close + 2 + LOOK_AHEAD)).find(|&j| c[j] == ')') {
                        let words: String = c[i + 1..close].iter().collect();
                        let addr: String = c[close + 2..end].iter().collect();
                        if words.trim() == addr.trim() || addr.trim().is_empty() {
                            o.push_str(&words);
                        } else {
                            o.push_str(&format!("{} ({})", words, addr.trim()));
                        }
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        o.push(ch);
        i += 1;
    }
    o
}

/// `![alt](address)` as a whole line: the address.
fn image_line(t: &str) -> Option<String> {
    let rest = t.trim().strip_prefix("![")?;
    let close = rest.find("](")?;
    let addr = rest[close + 2..].strip_suffix(')')?.trim();
    // an optional title: ![a](path "title")
    let addr = match addr.split_once(" \"") {
        Some((a, _)) if !addr.starts_with("data:") => a.trim(),
        _ => addr,
    };
    (!addr.is_empty()).then(|| addr.to_string())
}

fn heading_line(t: &str) -> Option<(u8, &str)> {
    let n = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&n) && t[n..].starts_with(' ') {
        Some((n as u8, t[n..].trim().trim_end_matches('#').trim()))
    } else {
        None
    }
}

fn list_line(line: &str) -> Option<(u8, String, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let t = &line[indent..];
    let level = (indent / 2).min(4) as u8;
    for m in ["- ", "* ", "+ "] {
        if let Some(r) = t.strip_prefix(m) {
            return Some((level, "\u{2022}".into(), r.trim()));
        }
    }
    let d = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=3).contains(&d) && (t[d..].starts_with(". ") || t[d..].starts_with(") ")) {
        return Some((level, format!("{}.", &t[..d]), t[d + 2..].trim()));
    }
    None
}

fn is_rule(t: &str) -> bool {
    let t: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

fn table_block(rows: &[&str]) -> Option<Block> {
    let mut cells: Vec<Vec<String>> = Vec::new();
    for r in rows {
        let t = r.trim().trim_start_matches('|').trim_end_matches('|');
        let row: Vec<String> = t.split('|').map(|c| strip_inline(c.trim())).collect();
        if row.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' '))) {
            continue;
        }
        cells.push(row);
    }
    if cells.is_empty() {
        return None;
    }
    let cols = cells.iter().map(Vec::len).max().unwrap_or(0).min(12);
    let widths: Vec<usize> = (0..cols).map(|i| cells.iter().filter_map(|r| r.get(i)).map(|c| c.chars().count()).max().unwrap_or(0).min(40)).collect();
    let lines: Vec<String> = cells
        .iter()
        .map(|r| {
            let mut l = String::new();
            for (i, w) in widths.iter().enumerate() {
                let c: String = r.get(i).map(|c| c.chars().take(40).collect()).unwrap_or_default();
                l.push_str(&format!("{c:<w$}  ", w = *w));
            }
            l.trim_end().to_string()
        })
        .collect();
    Some(Block::Code(lines.join("\n")))
}

fn flush(out: &mut Vec<Block>, para: &mut Vec<String>, quote: &mut Vec<String>) {
    if !para.is_empty() {
        out.push(Block::Para(para.join(" ")));
        para.clear();
    }
    if !quote.is_empty() {
        out.push(Block::Quote(quote.join(" ")));
        quote.clear();
    }
}

/// Blocks from Markdown-ish text. `picture` is asked for each picture address (in order, at most [`MAX_PICTURES`]); its error stops
/// the conversion and is returned, so a picture that cannot be had is never silently missing.
pub fn markdown_blocks(md: &str, picture: &mut dyn FnMut(&str) -> Result<PdfImage, String>) -> Result<Vec<Block>, String> {
    let mut out: Vec<Block> = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut quote: Vec<String> = Vec::new();
    let mut pics = 0usize;
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if out.len() > MAX_BLOCKS {
            return Err(format!("the text has too many parts (more than {MAX_BLOCKS})"));
        }
        let raw = lines[i].trim_end();
        let t = raw.trim();
        i += 1;
        if t.starts_with("```") || t.starts_with("~~~") {
            flush(&mut out, &mut para, &mut quote);
            let fence = &t[..3];
            let mut code = Vec::new();
            while i < lines.len() && !lines[i].trim().starts_with(fence) {
                code.push(lines[i].trim_end_matches('\r'));
                i += 1;
            }
            i += 1;
            out.push(Block::Code(code.join("\n")));
            continue;
        }
        if t.is_empty() {
            flush(&mut out, &mut para, &mut quote);
            continue;
        }
        if let Some(addr) = image_line(t) {
            flush(&mut out, &mut para, &mut quote);
            pics += 1;
            if pics > MAX_PICTURES {
                return Err(format!("too many pictures (at most {MAX_PICTURES})"));
            }
            out.push(Block::Image(picture(&addr)?));
            continue;
        }
        if let Some((n, h)) = heading_line(t) {
            flush(&mut out, &mut para, &mut quote);
            out.push(Block::Heading(n, strip_inline(h)));
            continue;
        }
        if is_rule(t) {
            flush(&mut out, &mut para, &mut quote);
            out.push(Block::Rule);
            continue;
        }
        if t.starts_with('|') {
            flush(&mut out, &mut para, &mut quote);
            let start = i - 1;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                i += 1;
            }
            if let Some(b) = table_block(&lines[start..i]) {
                out.push(b);
            }
            continue;
        }
        if let Some(q) = t.strip_prefix('>') {
            if !para.is_empty() {
                flush(&mut out, &mut para, &mut quote);
            }
            quote.push(strip_inline(q.trim()));
            continue;
        }
        if let Some((level, marker, text)) = list_line(raw) {
            flush(&mut out, &mut para, &mut quote);
            out.push(Block::Bullet(level, marker, strip_inline(text)));
            continue;
        }
        if !quote.is_empty() {
            flush(&mut out, &mut para, &mut quote);
        }
        para.push(strip_inline(t));
    }
    flush(&mut out, &mut para, &mut quote);
    for b in out.iter_mut() {
        match b {
            Block::Heading(_, t) | Block::Para(t) | Block::Quote(t) | Block::Code(t) => *t = fold(t),
            Block::Bullet(_, m, t) => {
                *m = fold(m);
                *t = fold(t);
            }
            _ => {}
        }
    }
    Ok(out)
}
