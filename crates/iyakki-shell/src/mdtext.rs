//! Markdown for the chat: the assistant answers in Markdown, and this turns it into laid-out lines that are drawn
//! with the shell's own fonts. It understands what models actually write: headings, **bold**, *italic*, `code`,
//! fenced code blocks, bullet and numbered lists (nested by indentation), > quotes, --- rules, [links](address) and
//! simple tables. Anything it does not recognise, or a marker that is never closed, is shown as it was written.
//! Nothing is fetched or run: a link is just coloured text.

use crate::gfx::{rgb, Face, Gfx};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Plain,
    Bold,
    Em,
    Code,
    Link,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Heading,
    Code,
    Rule,
    Quote,
    Gap,
}

#[derive(Clone, Debug)]
pub struct Piece {
    pub text: String,
    pub style: Style,
    pub x: f32,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub kind: Kind,
    pub y: f32,
    pub h: f32,
    pub size: f32,
    pub indent: f32,
    /// A list marker drawn left of the text (a bullet or `1.`).
    pub marker: Option<String>,
    pub pieces: Vec<Piece>,
}

pub struct Layout {
    pub lines: Vec<Line>,
    pub width: f32,
    pub height: f32,
}

fn face(style: Style, heading: bool) -> Face {
    match style {
        Style::Code => Face::Mono,
        Style::Bold => Face::Bold,
        _ if heading => Face::Bold,
        _ => Face::Sans,
    }
}

/// Splits one line of text into styled runs. Markers that are not closed stay as written.
pub fn inline(s: &str) -> Vec<(String, Style)> {
    let c: Vec<char> = s.chars().collect();
    let mut out: Vec<(String, Style)> = Vec::new();
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut Vec<(String, Style)>| {
        if !plain.is_empty() {
            out.push((std::mem::take(plain), Style::Plain));
        }
    };
    let find = |from: usize, pat: &[char]| -> Option<usize> { (from..c.len().saturating_sub(pat.len() - 1)).find(|&j| c[j..j + pat.len()] == *pat) };
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        let word_start = i == 0 || !c[i - 1].is_alphanumeric();
        if ch == '`' {
            if let Some(j) = find(i + 1, &['`']) {
                if j > i + 1 {
                    flush(&mut plain, &mut out);
                    out.push((c[i + 1..j].iter().collect(), Style::Code));
                    i = j + 1;
                    continue;
                }
            }
        } else if (ch == '*' || ch == '_') && c.get(i + 1) == Some(&ch) && c.get(i + 2).is_some_and(|x| !x.is_whitespace()) && (ch == '*' || word_start) {
            if let Some(j) = find(i + 2, &[ch, ch]) {
                if j > i + 2 && !c[j - 1].is_whitespace() {
                    flush(&mut plain, &mut out);
                    out.push((c[i + 2..j].iter().collect(), Style::Bold));
                    i = j + 2;
                    continue;
                }
            }
        } else if (ch == '*' || ch == '_') && c.get(i + 1).is_some_and(|x| !x.is_whitespace() && *x != ch) && word_start {
            if let Some(j) = find(i + 1, &[ch]) {
                let ends_word = c.get(j + 1).is_none_or(|x| !x.is_alphanumeric());
                if j > i + 1 && !c[j - 1].is_whitespace() && ends_word {
                    flush(&mut plain, &mut out);
                    out.push((c[i + 1..j].iter().collect(), Style::Em));
                    i = j + 1;
                    continue;
                }
            }
        } else if ch == '[' {
            if let Some(j) = find(i + 1, &[']']) {
                if c.get(j + 1) == Some(&'(') {
                    if let Some(k) = find(j + 2, &[')']) {
                        if j > i + 1 {
                            flush(&mut plain, &mut out);
                            out.push((c[i + 1..j].iter().collect(), Style::Link));
                            i = k + 1;
                            continue;
                        }
                    }
                }
            }
        }
        plain.push(ch);
        i += 1;
    }
    flush(&mut plain, &mut out);
    out
}

enum Block {
    Para { indent: usize, marker: Option<String>, text: String, quote: bool },
    Heading(usize, String),
    Code(Vec<String>),
    Rule,
    Gap,
}

fn is_rule(t: &str) -> bool {
    let t: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

fn blocks(md: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut code: Option<Vec<String>> = None;
    for raw in md.replace("\r\n", "\n").replace('\t', "    ").split('\n') {
        let t = raw.trim_start();
        if let Some(buf) = code.as_mut() {
            if t.starts_with("```") {
                out.push(Block::Code(code.take().unwrap_or_default()));
            } else {
                buf.push(raw.to_string());
            }
            continue;
        }
        if t.starts_with("```") {
            code = Some(Vec::new());
            continue;
        }
        let lead = raw.len() - t.len();
        if t.is_empty() {
            out.push(Block::Gap);
        } else if is_rule(t) {
            out.push(Block::Rule);
        } else if t.starts_with('#') && t.trim_start_matches('#').starts_with(' ') && t.len() - t.trim_start_matches('#').len() <= 6 {
            let n = t.len() - t.trim_start_matches('#').len();
            out.push(Block::Heading(n, t[n..].trim().trim_end_matches('#').trim().to_string()));
        } else if let Some(q) = t.strip_prefix('>') {
            out.push(Block::Para { indent: 0, marker: None, text: q.trim_start().to_string(), quote: true });
        } else if let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|m| t.strip_prefix(m)) {
            out.push(Block::Para { indent: lead / 2 + 1, marker: Some("•".into()), text: rest.to_string(), quote: false });
        } else if let Some((n, rest)) = t.split_once(". ").filter(|(n, _)| !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit())) {
            out.push(Block::Para { indent: lead / 2 + 1, marker: Some(format!("{n}.")), text: rest.to_string(), quote: false });
        } else if t.starts_with('|') && t.matches('|').count() >= 2 {
            let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
            if cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')) {
                continue; // the separator row
            }
            out.push(Block::Code(vec![cells.join("  │  ")]));
        } else {
            // a wrapped continuation of a list item keeps its indentation
            out.push(Block::Para { indent: if lead >= 2 { lead / 2 + 1 } else { 0 }, marker: None, text: t.to_string(), quote: false });
        }
    }
    if let Some(buf) = code {
        out.push(Block::Code(buf)); // a fence that was never closed (the answer is still arriving)
    }
    // adjacent single-line tables become one block
    let mut merged: Vec<Block> = Vec::new();
    for b in out {
        match (&b, merged.last_mut()) {
            (Block::Code(new), Some(Block::Code(prev))) if new.len() == 1 && !prev.is_empty() && prev.last().is_some_and(|l| l.contains('│')) && new[0].contains('│') => prev.push(new[0].clone()),
            _ => merged.push(b),
        }
    }
    merged
}

/// How many of `chars` fit in `max_w` (at least one, so a break always makes progress).
fn fit_chars(g: &mut Gfx, size: f32, face: Face, chars: &[char], max_w: f32) -> usize {
    let mut w = 0.0;
    let mut buf = [0u8; 4];
    for (i, c) in chars.iter().enumerate() {
        w += g.text_width(size, c.encode_utf8(&mut buf), face);
        if w > max_w {
            return i.max(1);
        }
    }
    chars.len()
}

/// Lays Markdown out in lines no wider than `max_w`, at `size` pixels.
pub fn layout(g: &mut Gfx, md: &str, size: f32, max_w: f32) -> Layout {
    layout_with(g, md, size, max_w, 1.08)
}

/// Like [`layout`] with the line spacing chosen: `spacing` times the font's own line height (1.08 is the compact default).
pub fn layout_with(g: &mut Gfx, md: &str, size: f32, max_w: f32, spacing: f32) -> Layout {
    let mut lines: Vec<Line> = Vec::new();
    let mut y = 0.0f32;
    let mut width = 0.0f32;
    let hang = size * 1.5;
    let lh = |g: &Gfx, sz: f32| g.line_height(sz, Face::Sans) * spacing;
    let mut prev_gap = true;
    for b in blocks(md) {
        match b {
            Block::Gap => {
                if !prev_gap {
                    let h = size * 0.5;
                    lines.push(Line { kind: Kind::Gap, y, h, size, indent: 0.0, marker: None, pieces: vec![] });
                    y += h;
                }
                prev_gap = true;
                continue;
            }
            Block::Rule => {
                let h = size * 0.9;
                lines.push(Line { kind: Kind::Rule, y, h, size, indent: 0.0, marker: None, pieces: vec![] });
                y += h;
                width = width.max(max_w * 0.5);
            }
            Block::Code(code) => {
                let sz = size * 0.88;
                let h = g.line_height(sz, Face::Mono) * 1.05;
                let pad = size * 0.35;
                for (n, l) in code.iter().enumerate() {
                    // long lines are broken by width, never cut off
                    let mut rest: Vec<char> = l.chars().collect();
                    let mut first = true;
                    loop {
                        let take = fit_chars(g, sz, Face::Mono, &rest, max_w - 2.0 * pad);
                        let head: String = rest[..take].iter().collect();
                        let w = g.text_width(sz, &head, Face::Mono) + 2.0 * pad;
                        width = width.max(w);
                        let top = n == 0 && first;
                        lines.push(Line { kind: Kind::Code, y: y + if top { pad * 0.5 } else { 0.0 }, h, size: sz, indent: pad, marker: None, pieces: vec![Piece { text: head, style: Style::Code, x: 0.0 }] });
                        y += h + if top { pad * 0.5 } else { 0.0 };
                        rest.drain(..take);
                        first = false;
                        if rest.is_empty() {
                            break;
                        }
                    }
                }
                y += pad * 0.5;
            }
            Block::Heading(level, text) => {
                let sz = size * match level { 1 => 1.38, 2 => 1.24, 3 => 1.12, _ => 1.04 };
                if !prev_gap {
                    y += size * 0.25;
                }
                wrap_into(g, &mut lines, &mut y, &mut width, &text, sz, 0.0, None, max_w, Kind::Heading, lh(g, sz));
                y += size * 0.1;
            }
            Block::Para { indent, marker, text, quote } => {
                let ind = if indent > 0 { indent as f32 * hang } else { 0.0 } + if quote { size * 0.9 } else { 0.0 };
                wrap_into(g, &mut lines, &mut y, &mut width, &text, size, ind, marker, max_w, if quote { Kind::Quote } else { Kind::Text }, lh(g, size));
            }
        }
        prev_gap = false;
    }
    while lines.last().is_some_and(|l| l.kind == Kind::Gap) {
        y -= lines.pop().map_or(0.0, |l| l.h);
    }
    Layout { lines, width, height: y }
}

#[allow(clippy::too_many_arguments)]
fn wrap_into(g: &mut Gfx, lines: &mut Vec<Line>, y: &mut f32, width: &mut f32, text: &str, sz: f32, indent: f32, marker: Option<String>, max_w: f32, kind: Kind, h: f32) {
    let heading = kind == Kind::Heading;
    // words with the style they were written in; a space in the source before a word is remembered
    let mut flagged: Vec<(String, Style, bool)> = Vec::new();
    let mut pending_space = false;
    for (t, st) in inline(text) {
        let parts: Vec<&str> = t.split(' ').collect();
        for (n, w) in parts.iter().enumerate() {
            if n > 0 {
                pending_space = true;
            }
            if w.is_empty() {
                continue;
            }
            flagged.push((w.to_string(), st, pending_space && !flagged.is_empty()));
            pending_space = false;
        }
    }
    let atoms = flagged;
    let fsz = |st: Style| if st == Style::Code { sz * 0.92 } else { sz };
    let space = g.text_width(sz, " ", Face::Sans);
    let avail = (max_w - indent).max(sz * 4.0);
    let mut cur: Vec<Piece> = Vec::new();
    let mut x = 0.0f32;
    let mut first = true;
    let push_line = |g: &mut Gfx, cur: &mut Vec<Piece>, x: &mut f32, first: &mut bool, lines: &mut Vec<Line>, y: &mut f32, width: &mut f32| {
        let _ = g;
        *width = width.max(indent + *x);
        lines.push(Line { kind, y: *y, h, size: sz, indent, marker: if *first { marker.clone() } else { None }, pieces: std::mem::take(cur) });
        *y += h;
        *x = 0.0;
        *first = false;
    };
    for (word, st, spaced) in atoms {
        let f = face(st, heading);
        let w = g.text_width(fsz(st), &word, f);
        let sp = if spaced && !cur.is_empty() { space } else { 0.0 };
        if x + sp + w > avail && !cur.is_empty() {
            push_line(g, &mut cur, &mut x, &mut first, lines, y, width);
        }
        let sp = if spaced && !cur.is_empty() { space } else { 0.0 };
        if w > avail {
            // one very long word (an address): break it by width
            let mut rest: Vec<char> = word.chars().collect();
            while !rest.is_empty() {
                let take = fit_chars(g, fsz(st), f, &rest, avail - x);
                let head: String = rest[..take].iter().collect();
                let hw = g.text_width(fsz(st), &head, f);
                cur.push(Piece { text: head, style: st, x });
                x += hw;
                rest.drain(..take);
                if !rest.is_empty() {
                    push_line(g, &mut cur, &mut x, &mut first, lines, y, width);
                }
            }
            continue;
        }
        x += sp;
        cur.push(Piece { text: word, style: st, x });
        x += w;
    }
    push_line(g, &mut cur, &mut x, &mut first, lines, y, width);
}

pub const CODE_BG: [u8; 4] = rgb(12, 16, 30);

/// Draws a layout with its top-left at (`x`, `y`). `color` is the plain text colour. Lines outside `clip` (top, bottom)
/// are skipped.
pub fn draw(g: &mut Gfx, l: &Layout, x: f32, y: f32, color: [u8; 4], clip: (f32, f32), full_w: f32) {
    for line in &l.lines {
        let ly = y + line.y;
        if ly + line.h < clip.0 || ly > clip.1 {
            continue;
        }
        let tx = x + line.indent;
        match line.kind {
            Kind::Gap => {}
            Kind::Rule => g.rect(x, ly + line.h * 0.5, full_w, 1.0, rgb(70, 82, 120)),
            Kind::Code => {
                g.rect(x - 4.0, ly, full_w + 8.0, line.h, CODE_BG);
                for p in &line.pieces {
                    g.text(tx + p.x, ly, line.size, rgb(190, 220, 255), &p.text, Face::Mono);
                }
            }
            _ => {
                if line.kind == Kind::Quote {
                    g.rect(x + 2.0, ly, 3.0, line.h, rgb(90, 110, 170));
                }
                if let Some(m) = &line.marker {
                    let mw = g.text_width(line.size, m, Face::Sans);
                    g.text(tx - mw - line.size * 0.35, ly, line.size, rgb(130, 160, 230), m, Face::Sans);
                }
                for p in &line.pieces {
                    let (col, f, sz) = match (p.style, line.kind) {
                        (Style::Code, _) => (rgb(255, 205, 120), Face::Mono, line.size * 0.92),
                        (Style::Bold, _) => (color, Face::Bold, line.size),
                        (Style::Em, _) => (rgb(205, 215, 240), Face::Sans, line.size),
                        (Style::Link, _) => (rgb(125, 175, 255), Face::Sans, line.size),
                        (_, Kind::Heading) => (rgb(255, 255, 255), Face::Bold, line.size),
                        _ => (if line.kind == Kind::Quote { rgb(175, 185, 210) } else { color }, Face::Sans, line.size),
                    };
                    if p.style == Style::Code {
                        let w = g.text_width(sz, &p.text, Face::Mono);
                        g.rrect(tx + p.x - 2.0, ly + 1.0, w + 4.0, line.h - 2.0, 4.0, CODE_BG);
                    }
                    let w = g.text(tx + p.x, ly, sz, col, &p.text, f);
                    if p.style == Style::Link {
                        g.rect(tx + p.x, ly + line.h - 3.0, w.max(1.0), 1.0, col);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay(md: &str, w: f32) -> (Gfx, Layout) {
        let mut g = Gfx::new(400, 300);
        let l = layout(&mut g, md, 16.0, w);
        (g, l)
    }

    fn texts(l: &Layout) -> Vec<String> {
        l.lines.iter().filter(|x| x.kind != Kind::Gap).map(|x| x.pieces.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(" ")).collect()
    }

    #[test]
    fn inline_styles_are_found_and_unclosed_markers_stay_as_written() {
        let r = inline("a **bold** and *it* and `code` and [site](https://x.test) end");
        let styled: Vec<(&str, Style)> = r.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        assert!(styled.contains(&("bold", Style::Bold)) && styled.contains(&("it", Style::Em)) && styled.contains(&("code", Style::Code)) && styled.contains(&("site", Style::Link)));
        assert!(!r.iter().any(|(t, _)| t.contains("https://x.test")), "the address is not shown");
        for open in ["**never closed", "snake_case_name and 2 * 3 * 4", "`open", "[a](b"] {
            assert_eq!(inline(open).iter().map(|(t, _)| t.as_str()).collect::<String>(), open, "{open}");
        }
        assert_eq!(inline("snake_case_name")[0].1, Style::Plain);
    }

    #[test]
    fn the_answer_from_the_screenshot_has_headings_lists_bold_and_a_rule() {
        let md = "1. **Two Sum** (Hash Table)  \n   - Find two numbers.  \n   - **Why**: Basics.\n\n---\n\n### **Caution**\n- Most APIs have **rate limits**.\n";
        let (_, l) = lay(md, 380.0);
        let t = texts(&l);
        assert_eq!(t[0], "Two Sum (Hash Table)", "{t:?}");
        assert!(l.lines[0].marker.as_deref() == Some("1."));
        assert_eq!(l.lines[1].marker.as_deref(), Some("•"));
        assert!(l.lines[1].indent > l.lines[0].indent, "nested items are indented");
        assert!(l.lines.iter().any(|x| x.kind == Kind::Rule));
        let h = l.lines.iter().find(|x| x.kind == Kind::Heading).unwrap();
        assert_eq!(h.pieces[0].text, "Caution");
        assert!(h.size > 16.0);
        assert!(!t.join("\n").contains("**") && !t.join("\n").contains("###"));
    }

    #[test]
    fn code_blocks_keep_their_lines_and_an_unclosed_fence_is_still_code() {
        let (_, l) = lay("text\n```\nlet a = 1;\n  let b = 2;\n```\nafter", 380.0);
        let code: Vec<&Line> = l.lines.iter().filter(|x| x.kind == Kind::Code).collect();
        assert_eq!(code.len(), 2);
        assert_eq!(code[1].pieces[0].text, "  let b = 2;", "leading spaces are kept");
        let (_, open) = lay("```\nstill arriving", 380.0);
        assert_eq!(open.lines.iter().filter(|x| x.kind == Kind::Code).count(), 1);
    }

    #[test]
    fn lines_wrap_inside_the_width_even_for_a_long_address_and_nothing_is_lost() {
        let url = "https://api.open-meteo.com/v1/forecast?latitude=37.7846&longitude=-122.4174&current_weather=true";
        let (mut g, l) = lay(&format!("see `{url}` now and then some more words to wrap around the edge"), 180.0);
        assert!(l.lines.len() >= 3);
        assert!(l.width <= 180.5, "{}", l.width);
        let joined: String = texts(&l).join("").replace(' ', "");
        assert!(joined.contains(&url.replace(' ', "")), "the address is intact when joined: {joined}");
        assert!(l.height > 0.0);
        draw(&mut g, &l, 4.0, 4.0, rgb(240, 240, 240), (0.0, 300.0), 180.0);
    }

    #[test]
    fn tables_render_as_aligned_rows_without_the_separator() {
        let (_, l) = lay("| Name | Why |\n|---|---|\n| Two Sum | hash |\n", 380.0);
        let t = texts(&l);
        assert_eq!(t.len(), 2, "{t:?}");
        assert!(t[0].contains("Name") && t[0].contains('│') && t[1].contains("Two Sum"));
    }

    #[test]
    fn empty_and_odd_input_do_not_panic() {
        for s in ["", "\n\n\n", "#", "# ", "-", "- ", "1. ", "|", "> ", "**", "`", "[]()", "___", "😀 **é** `ü`", &"x".repeat(5000)] {
            let (mut g, l) = lay(s, 120.0);
            draw(&mut g, &l, 0.0, 0.0, rgb(255, 255, 255), (0.0, 300.0), 120.0);
        }
    }
}
