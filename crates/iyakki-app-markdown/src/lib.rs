//! MARKDOWN: a read-only viewer for a Markdown text (F-SDK-14, `iyakki.markdown.view`). Blocks: headings, paragraphs, bullet and numbered
//! lists, quotes, code fences, rules. Inline marks are dropped (`*`, `_`, back-ticks, link and image syntax), links are listed under the
//! text by their words; a link to `javascript:`, `data:`, `vbscript:` or `file:` is counted and never listed, an image is shown as its
//! description. Nothing is fetched or opened: a link here is text. The text is cut at [`MAX_TEXT`] bytes.

use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{fit, Canvas, Hit, Role, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::TypeRole;
use iyakki_ui_sdk::tokens::{Radius, Space};
use iyakki_ui_sdk::widgets;

/// The app id the launcher, the dock and the policy would use.
pub const ID: &str = "markdown";
/// The most bytes of a document that are read, and the most links that are listed.
pub const MAX_TEXT: usize = 1 << 20;
pub const MAX_LINKS: usize = 500;

const A_TOP: u32 = 1;
const A_LINKS: u32 = 2;
const A_BODY: u32 = 3;
/// A row of the file list: `A_PICK + index`.
const A_PICK: u32 = 100;
/// The most rows of the file list the app takes.
pub const MAX_OFFERED: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, String),
    Para(String),
    Item(String),
    Quote(String),
    Code(Vec<String>),
    Rule,
}

/// What a document came to: the blocks, the safe links by their words, and how many unsafe links and images it had.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub blocks: Vec<Block>,
    pub links: Vec<(String, String)>,
    pub unsafe_links: usize,
    pub images: usize,
}

fn is_unsafe(url: &str) -> bool {
    let u: String = url.trim().chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    ["javascript:", "data:", "vbscript:", "file:"].iter().any(|p| u.starts_with(p))
}

/// The inline marks of one line removed; links and images are collected into `doc`.
fn inline(line: &str, doc: &mut Doc) -> String {
    let cs: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '\\' && i + 1 < cs.len() {
            out.push(cs[i + 1]);
            i += 2;
            continue;
        }
        let image = c == '!' && cs.get(i + 1) == Some(&'[');
        if c == '[' || image {
            let open = if image { i + 1 } else { i };
            if let Some(close) = (open + 1..cs.len()).find(|&j| cs[j] == ']') {
                if cs.get(close + 1) == Some(&'(') {
                    let mut depth = 0usize;
                    let end = (close + 2..cs.len()).find(|&j| match cs[j] {
                        '(' => {
                            depth += 1;
                            false
                        }
                        ')' if depth > 0 => {
                            depth -= 1;
                            false
                        }
                        ')' => true,
                        _ => false,
                    });
                    if let Some(end) = end {
                        let words: String = cs[open + 1..close].iter().collect();
                        let url: String = cs[close + 2..end].iter().collect();
                        if image {
                            doc.images += 1;
                            out.push_str(&format!("[image: {}]", if words.is_empty() { "no description" } else { &words }));
                        } else {
                            if is_unsafe(&url) {
                                doc.unsafe_links += 1;
                            } else if doc.links.len() < MAX_LINKS {
                                doc.links.push((words.clone(), url));
                            }
                            out.push_str(&words);
                        }
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        if c == '*' || c == '_' || c == '`' || c == '~' {
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Read a Markdown text into blocks (never fails: anything unknown is a paragraph).
pub fn parse(text: &str) -> Doc {
    let mut cut = text.len().min(MAX_TEXT);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut doc = Doc::default();
    let mut para = String::new();
    let mut code: Option<Vec<String>> = None;
    let flush = |para: &mut String, doc: &mut Doc| {
        if !para.is_empty() {
            let t = inline(para, doc);
            doc.blocks.push(Block::Para(t));
            para.clear();
        }
    };
    for raw in text[..cut].lines() {
        let line = raw.trim_end();
        if let Some(buf) = code.as_mut() {
            if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
                doc.blocks.push(Block::Code(code.take().unwrap_or_default()));
            } else {
                buf.push(line.to_string());
            }
            continue;
        }
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            flush(&mut para, &mut doc);
            code = Some(Vec::new());
        } else if t.is_empty() {
            flush(&mut para, &mut doc);
        } else if let Some(level) = (1..=6u8).find(|&n| t.starts_with(&"#".repeat(n as usize)) && t[n as usize..].starts_with(' ')) {
            flush(&mut para, &mut doc);
            let h = inline(t[level as usize..].trim().trim_end_matches('#').trim(), &mut doc);
            doc.blocks.push(Block::Heading(level, h));
        } else if t.len() >= 3 && t.chars().all(|c| c == '-' || c == '*' || c == '_') && t.chars().all(|c| c == t.chars().next().unwrap_or('-')) {
            flush(&mut para, &mut doc);
            doc.blocks.push(Block::Rule);
        } else if let Some(rest) = t.strip_prefix("> ").or_else(|| t.strip_prefix('>')) {
            flush(&mut para, &mut doc);
            let q = inline(rest.trim(), &mut doc);
            doc.blocks.push(Block::Quote(q));
        } else if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ ")) {
            flush(&mut para, &mut doc);
            let it = inline(rest.trim(), &mut doc);
            doc.blocks.push(Block::Item(format!("\u{2022} {it}")));
        } else if let Some((n, rest)) = t.split_once(". ").filter(|(n, _)| !n.is_empty() && n.len() < 6 && n.chars().all(|c| c.is_ascii_digit())) {
            flush(&mut para, &mut doc);
            let it = inline(rest.trim(), &mut doc);
            doc.blocks.push(Block::Item(format!("{n}. {it}")));
        } else {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(t);
        }
    }
    if let Some(buf) = code {
        doc.blocks.push(Block::Code(buf));
    }
    flush(&mut para, &mut doc);
    doc
}

pub struct MarkdownApp {
    pub title: String,
    pub doc: Doc,
    /// First visible line (the document laid out as lines at the width of the last draw).
    pub top: usize,
    pub show_links: bool,
    lines: usize,
    page: usize,
    /// The names the person may choose from (`markdown.openFile`): the app never sees where a file is, only these names.
    offered: Option<Vec<String>>,
    sel: usize,
    chosen: Option<usize>,
}

impl Default for MarkdownApp {
    fn default() -> Self {
        MarkdownApp::new("Markdown", "")
    }
}

impl MarkdownApp {
    pub fn new(title: &str, text: &str) -> MarkdownApp {
        MarkdownApp { title: title.chars().filter(|c| !c.is_control()).take(60).collect(), doc: parse(text), top: 0, show_links: false, lines: 0, page: 10, offered: None, sel: 0, chosen: None }
    }

    /// Show another document (what `markdown.view` does).
    pub fn open(&mut self, title: &str, text: &str) {
        *self = MarkdownApp::new(title, text);
    }

    /// Show a list of file names to choose one from (at most [`MAX_OFFERED`]). The choice comes back through [`MarkdownApp::take_choice`].
    pub fn offer(&mut self, names: Vec<String>) {
        let names: Vec<String> = names.into_iter().take(MAX_OFFERED).map(|n| n.chars().filter(|c| !c.is_control()).take(120).collect()).collect();
        self.sel = 0;
        self.chosen = None;
        self.offered = if names.is_empty() { None } else { Some(names) };
    }

    /// Whether the file list is showing.
    pub fn choosing(&self) -> bool {
        self.offered.is_some()
    }

    /// The index of the name the person chose, once (the host reads the file and calls [`MarkdownApp::open`]).
    pub fn take_choice(&mut self) -> Option<usize> {
        self.chosen.take()
    }

    fn scroll(&mut self, by: isize) {
        let max = self.lines.saturating_sub(1) as isize;
        self.top = (self.top as isize + by).clamp(0, max.max(0)) as usize;
    }
}

/// Break `s` into lines no wider than `w`, at spaces where it can (a word longer than a line is cut).
fn wrap(c: &mut dyn Canvas, size: f32, style: TextStyle, s: &str, w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let trial = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if c.measure(size, &trial, style) <= w {
            cur = trial;
            continue;
        }
        if !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
        }
        let mut rest = word.to_string();
        while c.measure(size, &rest, style) > w && rest.chars().count() > 1 {
            let mut n = rest.chars().count() - 1;
            while n > 1 && c.measure(size, &rest.chars().take(n).collect::<String>(), style) > w {
                n -= 1;
            }
            lines.push(rest.chars().take(n).collect());
            rest = rest.chars().skip(n).collect();
        }
        cur = rest;
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

impl UiApp for MarkdownApp {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> String {
        "MARKDOWN".into()
    }

    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        if let Some(names) = &self.offered {
            let n = names.len();
            match ev {
                UiEvent::Key(UiKey::Up) => self.sel = self.sel.saturating_sub(1),
                UiEvent::Key(UiKey::Down) => self.sel = (self.sel + 1).min(n - 1),
                UiEvent::Key(UiKey::Enter) => {
                    self.chosen = Some(self.sel);
                    self.offered = None;
                }
                UiEvent::Action(a) if *a >= A_PICK && ((*a - A_PICK) as usize) < n => {
                    self.chosen = Some((*a - A_PICK) as usize);
                    self.offered = None;
                }
                // Esc closes the list first and leaves the viewer on the next press
                UiEvent::Key(UiKey::Esc) => self.offered = None,
                _ => {}
            }
            return Flow::Stay;
        }
        match ev {
            UiEvent::Key(UiKey::Esc) => return Flow::Back,
            UiEvent::Key(UiKey::Up) => self.scroll(-1),
            UiEvent::Key(UiKey::Down) => self.scroll(1),
            UiEvent::Key(UiKey::Char(' ')) => self.scroll(self.page as isize),
            UiEvent::Key(UiKey::Char('b')) => self.scroll(-(self.page as isize)),
            UiEvent::Key(UiKey::Char('l')) => self.show_links = !self.show_links,
            UiEvent::Wheel(d) => self.scroll(if *d > 0 { -3 } else { 3 }),
            UiEvent::Action(A_TOP) => self.top = 0,
            UiEvent::Action(A_LINKS) => self.show_links = !self.show_links,
            _ => {}
        }
        Flow::Stay
    }

    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let th = cx.colors();
        let m = cx.metrics();
        c.gradient(Rect::new(0.0, 0.0, w, h), th.bg_top, th.bg_bottom);
        let g = m.gutter();
        let body = Rect::new(g, g, (w - 2.0 * g).max(0.0), (h - 2.0 * g).max(0.0));
        let ts = m.size(TypeRole::Title);
        let title = fit(c, ts, &self.title, TextStyle::Bold, body.w);
        c.text(body.x, body.y, ts, th.text, &title, TextStyle::Bold);
        if let Some(names) = &self.offered {
            // the list of files: one row each, the one in focus marked, the list scrolled to keep it in view
            c.text(body.x, body.y + m.leading(ts), m.size(TypeRole::Body), th.muted, "Choose a file (Up, Down, Enter; Esc closes)", TextStyle::Regular);
            let row_h = m.min_target().max(m.leading(m.size(TypeRole::Label)) * 1.3);
            let top_y = body.y + m.leading(ts) + m.leading(m.size(TypeRole::Body)) * 1.3;
            let room = (body.bottom() - top_y).max(row_h);
            let fit_rows = ((room / row_h).floor() as usize).max(1);
            let first = self.sel.saturating_sub(fit_rows - 1);
            for (k, name) in names.iter().enumerate().skip(first).take(fit_rows) {
                let r = Rect::new(body.x, top_y + (k - first) as f32 * row_h, body.w, row_h - m.gap(Space::Xs));
                let on = k == self.sel;
                widgets::surface(c, th, &m, r, if on { th.accent } else { th.card }, Radius::Md, false);
                let t = fit(c, m.size(TypeRole::Label), name, TextStyle::Regular, r.w - 2.0 * m.gap(Space::Md));
                c.text(r.x + m.gap(Space::Md), r.y + (r.h - m.size(TypeRole::Label)) / 2.0, m.size(TypeRole::Label), th.text, &t, TextStyle::Regular);
                c.hit(r, Hit { action: A_PICK + k as u32, name: name.clone(), role: Role::ListItem, ..Default::default() });
            }
            return;
        }
        let bar_h = m.min_target();
        let bar = Rect::new(body.x, body.bottom() - bar_h, body.w, bar_h);
        let text_area = Rect::new(body.x, body.y + m.leading(ts), body.w, (bar.y - body.y - m.leading(ts) - m.gap(Space::Md)).max(0.0));
        // lay the document out as (text, size, style, colour, indent) lines
        let tw = text_area.w.max(m.min_target());
        let base = m.size(TypeRole::Label);
        let mut lines: Vec<(String, f32, TextStyle, iyakki_ui_sdk::geom::Color, f32)> = Vec::new();
        if self.show_links {
            lines.push((format!("Links ({})", self.doc.links.len()), m.size(TypeRole::Large), TextStyle::Bold, th.text, 0.0));
            for (words, url) in &self.doc.links {
                for l in wrap(c, base, TextStyle::Regular, &format!("{words} -> {url}"), tw) {
                    lines.push((l, base, TextStyle::Regular, th.accent, 0.0));
                }
            }
            if self.doc.links.is_empty() {
                lines.push(("No links in this document.".into(), base, TextStyle::Regular, th.muted, 0.0));
            }
            if self.doc.unsafe_links > 0 {
                lines.push((format!("{} link(s) left out: they did not point to a web address.", self.doc.unsafe_links), base, TextStyle::Regular, th.muted, 0.0));
            }
        } else {
            for b in &self.doc.blocks {
                match b {
                    Block::Heading(level, t) => {
                        let size = m.size(match level { 1 => TypeRole::Display, 2 => TypeRole::Title, 3 => TypeRole::Large, _ => TypeRole::Label });
                        lines.push((String::new(), base / 2.0, TextStyle::Regular, th.text, 0.0));
                        for l in wrap(c, size, TextStyle::Bold, t, tw) {
                            lines.push((l, size, TextStyle::Bold, th.text, 0.0));
                        }
                    }
                    Block::Para(t) => {
                        for l in wrap(c, base, TextStyle::Regular, t, tw) {
                            lines.push((l, base, TextStyle::Regular, th.text, 0.0));
                        }
                        lines.push((String::new(), base / 2.0, TextStyle::Regular, th.text, 0.0));
                    }
                    Block::Item(t) => {
                        for (i, l) in wrap(c, base, TextStyle::Regular, t, tw - m.gap(Space::Xl)).into_iter().enumerate() {
                            lines.push((l, base, TextStyle::Regular, th.text, if i == 0 { m.gap(Space::Md) } else { m.gap(Space::Xl) + m.gap(Space::Xs) }));
                        }
                    }
                    Block::Quote(t) => {
                        for l in wrap(c, base, TextStyle::Regular, t, tw - m.gap(Space::Xl) - m.gap(Space::Xs)) {
                            lines.push((l, base, TextStyle::Regular, th.muted, m.gap(Space::Xl)));
                        }
                    }
                    Block::Code(rows) => {
                        let cs = m.size(TypeRole::Small);
                        for r in rows {
                            let r = fit(c, cs, r, TextStyle::Mono, tw - m.gap(Space::Lg));
                            lines.push((r, cs, TextStyle::Mono, th.muted, m.gap(Space::Lg)));
                        }
                        lines.push((String::new(), base / 2.0, TextStyle::Regular, th.text, 0.0));
                    }
                    Block::Rule => lines.push(("\u{2014}".repeat(12), base, TextStyle::Regular, th.dim, 0.0)),
                }
            }
            if self.doc.blocks.is_empty() {
                lines.push(("This document is empty.".into(), base, TextStyle::Regular, th.muted, 0.0));
            }
        }
        self.lines = lines.len();
        self.top = self.top.min(lines.len().saturating_sub(1));
        // draw from `top` until the area is full
        let leading = |size: f32| m.leading(size);
        let mut y = text_area.y;
        let mut shown: usize = 0;
        let top = self.top;
        c.clipped(text_area, &mut |cc| {
            for (t, size, style, col, indent) in lines.iter().skip(top) {
                let lh = leading(*size);
                if y + lh > text_area.bottom() {
                    break;
                }
                if !t.is_empty() {
                    cc.text(text_area.x + indent, y, *size, *col, t, *style);
                }
                y += lh;
                shown += 1;
            }
        });
        self.page = shown.saturating_sub(1).max(1);
        // the whole text area is one named region for a screen reader (the words themselves are drawn as text)
        c.hit(text_area, Hit { action: A_BODY, name: format!("{}: {} lines, from line {}", self.title, self.lines, self.top + 1), role: Role::Text, ..Default::default() });
        // bottom bar: top, links
        let bw = ((bar.w - m.gap(Space::Md)) / 2.0).max(0.0);
        widgets::button(c, th, &m, Rect::new(bar.x, bar.y, bw, bar.h), "Top", A_TOP, false);
        widgets::button(c, th, &m, Rect::new(bar.x + bw + m.gap(Space::Md), bar.y, bw, bar.h), if self.show_links { "Text" } else { "Links" }, A_LINKS, false);
    }
}

#[cfg(test)]
mod tests;
