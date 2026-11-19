//! A page (`iyakki_web::Doc`) laid out as lines of words and drawn as an SDK display list. Reader-style: headings, paragraphs, lists,
//! quotes, preformatted text, rules, tables as one line per row; links are hit regions; pictures and controls are named in words.
//! Scripts and styles of the page are not run or applied (the in-house engine is a reader). The frame holds only what is in the view.

use iyakki_core_api::browsersession::{FindState, PageDefaults, PageFont, Palette, Scheme};
use iyakki_ui_sdk::canvas::{Canvas, Hit, Role, TextStyle};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::list::{approx_measure, ListCanvas};
use iyakki_web::doc::{Block, Doc, TextBlock};

/// More words than this and the rest of the page is not laid out (the view says so at the end).
pub const MAX_WORDS: usize = 40_000;
/// Link regions stop here (the SDK view reserves this many action numbers for the page).
pub const MAX_LINK_ACTIONS: usize = 8_000;
const PAD: f32 = 10.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub x: f32,
    pub w: f32,
    pub text: String,
    pub size: f32,
    pub style: TextStyle,
    pub link: Option<usize>,
    pub code: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub y: f32,
    pub h: f32,
    pub runs: Vec<Run>,
    /// A horizontal rule instead of text.
    pub rule: bool,
    /// Preformatted: drawn on a tinted band.
    pub pre: bool,
}

impl Line {
    /// The text of the line as find sees it (words joined by one space) and each run's start offset in it (in characters).
    pub fn text_with_offsets(&self) -> (String, Vec<usize>) {
        let (mut s, mut at, mut n) = (String::new(), Vec::new(), 0usize);
        for (i, r) in self.runs.iter().enumerate() {
            if i > 0 {
                s.push(' ');
                n += 1;
            }
            at.push(n);
            s.push_str(&r.text);
            n += r.text.chars().count();
        }
        (s, at)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub lines: Vec<Line>,
    pub height: f32,
    pub truncated: bool,
}

struct Pen {
    width: f32,
    ts: f32,
    /// The page's default text size and whether the default family is the mono one.
    base: f32,
    mono: bool,
    y: f32,
    words: usize,
    out: Vec<Line>,
    truncated: bool,
}

fn measure(size: f32, s: &str, style: TextStyle) -> f32 {
    approx_measure(size, s, style)
}

impl Pen {
    fn full(&self) -> bool {
        self.words >= MAX_WORDS
    }

    /// Wrap `words` (text, style, link) into lines of at most `max_w` starting at `x0`.
    fn flow(&mut self, x0: f32, max_w: f32, size: f32, words: &[(String, TextStyle, Option<usize>)], code: bool) {
        let space = measure(size, " ", TextStyle::Regular);
        let lh = size * 1.45;
        let mut runs: Vec<Run> = Vec::new();
        let mut x = x0;
        for (t, style, link) in words {
            if self.full() {
                self.truncated = true;
                break;
            }
            let w = measure(size, t, *style);
            if x > x0 && x + w > x0 + max_w {
                self.out.push(Line { y: self.y, h: lh, runs: std::mem::take(&mut runs), rule: false, pre: false });
                self.y += lh;
                x = x0;
            }
            runs.push(Run { x, w, text: t.clone(), size, style: *style, link: *link, code });
            x += w + space;
            self.words += 1;
        }
        if !runs.is_empty() {
            self.out.push(Line { y: self.y, h: lh, runs, rule: false, pre: false });
            self.y += lh;
        }
    }

    fn text_block(&mut self, doc: &Doc, t: &TextBlock, indent: f32) {
        let base = self.base * self.ts;
        let size = match t.level {
            1 => base * 1.7,
            2 => base * 1.45,
            3 => base * 1.25,
            4..=6 => base * 1.1,
            _ => base,
        };
        let head = t.level > 0;
        let x0 = PAD + indent + f32::from(t.indent) * 18.0 + if t.quote { 14.0 } else { 0.0 };
        let max_w = (self.width - x0 - PAD).max(20.0);
        let mut words: Vec<(String, TextStyle, Option<usize>)> = Vec::new();
        if !t.marker.is_empty() {
            words.push((t.marker.clone(), TextStyle::Regular, None));
        }
        for s in &t.spans {
            let style = if head || s.style.bold { TextStyle::Bold } else if s.style.code || self.mono { TextStyle::Mono } else { TextStyle::Regular };
            let text = if let Some(i) = s.image {
                format!("[picture: {}]", doc.images.get(i).map_or("", |im| if im.alt.is_empty() { "no description" } else { im.alt.as_str() }))
            } else if s.control.is_some() {
                "[form control]".to_string()
            } else {
                s.text.clone()
            };
            for w in text.split_whitespace() {
                words.push((w.to_string(), style, s.link));
            }
        }
        if words.is_empty() {
            return;
        }
        if head {
            self.y += size * 0.5;
        }
        self.flow(x0, max_w, size, &words, false);
        self.y += if head { size * 0.3 } else { size * 0.45 };
    }

    fn blocks(&mut self, doc: &Doc, blocks: &[Block], indent: f32, depth: usize) {
        if depth > 24 {
            return;
        }
        for b in blocks {
            if self.full() {
                self.truncated = true;
                return;
            }
            match b {
                Block::Text(t) => self.text_block(doc, t, indent),
                Block::Box(k) => self.blocks(doc, &k.children, indent, depth + 1),
                Block::Row(r) => {
                    for c in &r.cols {
                        self.blocks(doc, &c.blocks, indent, depth + 1);
                    }
                }
                Block::Pre(p) => {
                    let size = self.base * 13.0 / 15.0 * self.ts;
                    let lh = size * 1.4;
                    for l in p.lines().take(2000) {
                        let l: String = l.chars().take(400).collect();
                        let w = measure(size, &l, TextStyle::Mono);
                        let runs = if l.is_empty() { Vec::new() } else { vec![Run { x: PAD + indent + 6.0, w, text: l, size, style: TextStyle::Mono, link: None, code: true }] };
                        self.out.push(Line { y: self.y, h: lh, runs, rule: false, pre: true });
                        self.y += lh;
                        self.words += 1;
                    }
                    self.y += size * 0.5;
                }
                Block::Rule => {
                    self.out.push(Line { y: self.y + 6.0, h: 12.0, runs: Vec::new(), rule: true, pre: false });
                    self.y += 14.0;
                }
                Block::Table(t) => {
                    for row in t.rows.iter().take(500) {
                        let cells: Vec<String> = row.iter().map(|c| c.text()).filter(|c| !c.is_empty()).collect();
                        if cells.is_empty() {
                            continue;
                        }
                        let words: Vec<(String, TextStyle, Option<usize>)> = cells.join(" | ").split_whitespace().map(|w| (w.to_string(), TextStyle::Regular, None)).collect();
                        self.flow(PAD + indent, (self.width - 2.0 * PAD - indent).max(20.0), self.base * self.ts, &words, false);
                    }
                    self.y += 8.0 * self.ts;
                }
            }
        }
    }
}

/// Lay the page out for a view `width` wide.
pub fn layout(doc: &Doc, width: f32, text_scale: f32, d: &PageDefaults) -> Layout {
    let base = if d.font_size.is_finite() { d.font_size.clamp(8.0, 40.0) } else { 15.0 };
    let mut p = Pen { width, ts: text_scale.clamp(0.5, 3.0), base, mono: d.font == PageFont::Mono, y: PAD, words: 0, out: Vec::new(), truncated: false };
    p.blocks(doc, &doc.blocks, 0.0, 0);
    if p.truncated || doc.truncated {
        let size = p.base * 13.0 / 15.0 * p.ts;
        let w = vec![("The rest of this page was not shown.".to_string(), TextStyle::Regular, None)];
        p.words = 0;
        p.flow(PAD, (width - 2.0 * PAD).max(20.0), size, &w, false);
    }
    Layout { height: p.y + PAD, lines: p.out, truncated: p.truncated || doc.truncated }
}

/// Every match of `query` (case-insensitive) as (line index, start char, end char), in reading order.
pub fn find_all(l: &Layout, query: &str) -> Vec<(usize, usize, usize)> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, line) in l.lines.iter().enumerate() {
        let (text, _) = line.text_with_offsets();
        let t: Vec<char> = text.to_lowercase().chars().collect();
        if t.len() < q.len() {
            continue;
        }
        let mut s = 0;
        while s + q.len() <= t.len() {
            if t[s..s + q.len()] == q[..] {
                out.push((i, s, s + q.len()));
                s += q.len();
            } else {
                s += 1;
            }
        }
    }
    out
}

/// What the frame needs besides the layout.
pub struct FrameIn<'a> {
    pub layout: &'a Layout,
    pub width: f32,
    pub height: f32,
    pub palette: Palette,
    pub scroll: f32,
    pub find: &'a FindState,
    pub matches: &'a [(usize, usize, usize)],
    /// A line shown on the top of the page instead of it (loading, an error, a note).
    pub banner: Option<&'a str>,
    pub link_count: usize,
    pub defaults: PageDefaults,
    pub text_scale: f32,
}

/// The display list of the visible part of the page.
pub fn frame(i: &FrameIn) -> String {
    let (w, h, pal) = (i.width, i.height, i.palette);
    let mut c = ListCanvas::new(w, h);
    let hc = i.defaults.scheme == Scheme::HighContrast;
    let line_w = if hc { 2.0 } else { 1.0 };
    c.rect(Rect::new(0.0, 0.0, w, h), pal.bg);
    let view = Rect::new(0.0, 0.0, w, h);
    c.clipped(view, &mut |c| {
        let mut top = 0.0;
        if let Some(b) = i.banner {
            let size = 14.0 * i.text_scale.clamp(0.5, 3.0);
            c.text(PAD, 8.0, size, if hc { pal.text } else { pal.muted }, b, TextStyle::Regular);
            top = 8.0 + size * 1.6;
        }
        for (li, line) in i.layout.lines.iter().enumerate() {
            let y = line.y - i.scroll + top;
            if y + line.h < 0.0 || y > h {
                continue;
            }
            if line.rule {
                c.line(PAD, y + 6.0, w - PAD, y + 6.0, line_w, if hc { pal.text } else { pal.rule });
                continue;
            }
            if line.pre {
                c.rect(Rect::new(PAD, y, (w - 2.0 * PAD).max(0.0), line.h), pal.code_bg);
            }
            let (_, offs) = line.text_with_offsets();
            for (ri, r) in line.runs.iter().enumerate() {
                let start = offs[ri];
                let end = start + r.text.chars().count();
                if let Some(mi) = i.matches.iter().position(|(l, s, e)| *l == li && *s < end && *e > start) {
                    let cur = mi + 1 == i.find.current;
                    let mut col = pal.mark;
                    if !cur && !hc {
                        col[3] = 110;
                    }
                    if hc {
                        c.outline(Rect::new(r.x - 1.0, y, r.w + 2.0, line.h), 0.0, if cur { 3.0 } else { 1.5 }, pal.text);
                    }
                    c.rect(Rect::new(r.x - 1.0, y, r.w + 2.0, line.h), col);
                }
                let color = if r.link.is_some() { pal.link } else { pal.text };
                c.text(r.x, y + (line.h - r.size) / 2.0, r.size, color, &r.text, r.style);
                if let Some(l) = r.link {
                    if l < i.link_count.min(MAX_LINK_ACTIONS) {
                        c.line(r.x, y + line.h - 2.0, r.x + r.w, y + line.h - 2.0, line_w, pal.link);
                        c.hit(Rect::new(r.x, y, r.w, line.h), Hit { action: l as u32 + 1, name: r.text.clone(), role: Role::Link });
                    }
                }
            }
        }
        // a thin scroll bar when the page is longer than the view
        let total = i.layout.height + top;
        if total > h {
            let th = (h * h / total).max(24.0).min(h);
            let ty = (i.scroll / (total - h).max(1.0)) * (h - th);
            c.rrect(Rect::new(w - if hc { 8.0 } else { 5.0 }, ty, if hc { 6.0 } else { 3.0 }, th), 1.5, if hc { pal.text } else { pal.rule });
        }
    });
    c.finish().to_text()
}
