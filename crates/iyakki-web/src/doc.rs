//! From a parsed page to what is shown: blocks of styled text.

use crate::css::{Length, Rgba};
use std::collections::HashMap;

use crate::domtree::PseudoEl;
use crate::html::{self, NodeKind, Tree};
use crate::style::{self, Align, Computed, Display, Fit, Float, Fx, Justify, StyleSheets, Transform, VAlign};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// Stands in for something that is not shown (an image).
    pub dim: bool,
    /// From style sheets: the text colour, a background behind the words, underline (`Some(false)` removes the
    /// underline links have), strike-through, and the size as a percentage of the base size (0: the block's own).
    pub color: Option<Rgba>,
    pub bg: Option<Rgba>,
    pub underline: Option<bool>,
    pub strike: bool,
    pub size_pct: u16,
    /// A span with no text and this many pixels: the space an inline element's padding and margin leave beside its
    /// words (`bg` is painted behind it when it is padding).
    pub gap: i16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    /// Index into [`Doc::links`].
    pub link: Option<usize>,
    /// A form control drawn in the text here (index into [`Doc::controls`]); `text` is empty.
    pub control: Option<usize>,
    /// A picture drawn in the text here (index into [`Doc::images`]); `text` is empty.
    pub image: Option<usize>,
    /// An atomic inline box (`display:inline-block`) in the text here (index into [`Doc::inlines`]); `text` is empty.
    pub inline: Option<usize>,
}

/// An `inline-block`: a box with blocks inside that sits in a line of text like one big word.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineBox {
    pub style: BoxStyle,
    pub height: Length,
    pub min_height: Length,
    pub valign: VAlign,
    pub children: Vec<Block>,
}

/// An `<img>` on the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    /// As written (resolve against the page's address); may be a `data:` address.
    pub src: String,
    pub alt: String,
    /// Sizes from the `width` and `height` attributes, in pixels.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// `float` and `object-fit` from style sheets.
    pub float: Float,
    pub fit: Fit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    Text,
    Password,
    Checkbox,
    Radio,
    Submit,
    Button,
    Hidden,
    Select,
    Textarea,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub kind: ControlKind,
    /// Index into [`Doc::forms`].
    pub form: Option<usize>,
    pub name: String,
    /// What is sent (checkbox and radio value, a submit button's value) or the initial text.
    pub value: String,
    /// The words on a button.
    pub label: String,
    pub placeholder: String,
    pub checked: bool,
    pub options: Vec<SelectOption>,
    /// Width in characters (text fields) and lines (text areas).
    pub size: u32,
    pub rows: u32,
    pub disabled: bool,
    pub readonly: bool,
    /// Size from style sheets, and the margin around it (pixels; top, right, bottom, left).
    pub width: Length,
    pub height: Length,
    pub margin: [f32; 4],
    pub valign: VAlign,
    /// The node of the page's tree this control was made from (a script engine that keeps the page alive acts on it).
    pub node: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    /// As written on the page (resolve against the page's address); empty means the page itself.
    pub action: String,
    pub post: bool,
    pub enctype: String,
}

/// A box from style sheets around some blocks: margins, padding, border, background and width.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxStyle {
    /// top, right, bottom, left (pixels)
    pub margin: [f32; 4],
    pub auto_left: bool,
    pub auto_right: bool,
    pub padding: [f32; 4],
    pub border_w: [f32; 4],
    pub border_c: [Rgba; 4],
    pub radius: f32,
    pub bg: Option<Rgba>,
    pub width: Length,
    pub max_width: Length,
    pub min_width: Length,
    /// Floats, positioning, shadow, gradient and opacity.
    pub fx: Fx,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoxBlock {
    pub style: BoxStyle,
    pub children: Vec<Block>,
}

/// One column of a row: what a flex item or grid cell holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub style: BoxStyle,
    /// `flex-grow`, and the width asked for (`flex-basis`, else `width`).
    pub grow: f32,
    pub basis: Length,
    pub blocks: Vec<Block>,
}

/// Blocks side by side (`display:flex` in a row, or a grid).
#[derive(Clone, Debug, PartialEq)]
pub struct RowBlock {
    pub style: BoxStyle,
    pub gap: f32,
    pub row_gap: f32,
    pub justify: Justify,
    pub wrap: bool,
    pub grid: bool,
    /// Grid columns (0: as many as fit, each at least `grid_min` wide).
    pub grid_cols: u32,
    pub grid_min: f32,
    pub cols: Vec<Column>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextBlock {
    /// 0 = text, 1..=6 = heading.
    pub level: u8,
    /// List nesting depth.
    pub indent: u8,
    pub quote: bool,
    /// "•" or "3." for list items.
    pub marker: String,
    pub spans: Vec<Span>,
    /// From style sheets.
    pub align: Align,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text(TextBlock),
    Box(BoxBlock),
    Row(RowBlock),
    Pre(String),
    Rule,
    Table(Table),
}

/// A `<table>`: rows of cells, each holding blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub rows: Vec<Vec<Cell>>,
    /// The `width` attribute or style (auto: as wide as the content needs).
    pub width: Length,
    /// Where a table narrower than its column sits (`<center>`, `align=center`, `margin:auto`).
    pub align: Align,
    pub bg: Option<Rgba>,
    /// `cellspacing`, in pixels.
    pub spacing: f32,
    /// Draw lines between the rows (a `border`, header cells, or borders on the cells).
    pub lines: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub blocks: Vec<Block>,
    pub colspan: u32,
    pub rowspan: u32,
    pub width: Length,
    pub nowrap: bool,
    pub valign: VAlign,
    /// top, right, bottom, left (pixels)
    pub pad: [f32; 4],
    pub bg: Option<Rgba>,
}

impl Cell {
    /// The words in the cell, for the reader and for tests.
    pub fn text(&self) -> String {
        fn walk(b: &[Block], out: &mut Vec<String>) {
            for x in b {
                match x {
                    Block::Text(t) => out.push(t.spans.iter().map(|s| s.text.as_str()).collect()),
                    Block::Pre(p) => out.push(p.clone()),
                    Block::Box(k) => walk(&k.children, out),
                    Block::Row(r) => r.cols.iter().for_each(|c| walk(&c.blocks, out)),
                    Block::Table(t) => t.rows.iter().for_each(|r| r.iter().for_each(|c| out.push(c.text()))),
                    Block::Rule => {}
                }
            }
        }
        let mut lines = Vec::new();
        walk(&self.blocks, &mut lines);
        lines.retain(|l| !l.trim().is_empty());
        lines.join(" ")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub title: String,
    /// The background the page asked for (on `html` or `body`); the whole view is painted with it.
    pub canvas: Option<Rgba>,
    pub blocks: Vec<Block>,
    /// The `href` of every link, as written on the page (resolve against the page's address to follow one).
    pub links: Vec<String>,
    pub controls: Vec<Control>,
    pub forms: Vec<Form>,
    pub images: Vec<ImageRef>,
    pub inlines: Vec<InlineBox>,
    pub truncated: bool,
    /// What a screen reader needs to know about the page (roles, names, states), made from the same tree; `None` for documents that
    /// were not made from a page (the reader view, plain text).
    pub access: Option<std::sync::Arc<crate::a11y::A11yTree>>,
}

const MAX_BLOCKS: usize = 20_000;
const MAX_TEXT: usize = 2_000_000;
const SKIP: [&str; 11] = ["script", "style", "noscript", "template", "svg", "head", "title", "iframe", "object", "embed", "canvas"];
const BLOCKISH: [&str; 17] = ["p", "div", "section", "article", "header", "footer", "nav", "aside", "main", "figure", "figcaption", "address", "details", "summary", "fieldset", "dl", "center"];

/// Tags that are blocks by default (they can carry a box from style sheets).
const BLOCK_TAGS: [&str; 33] = [
    "div", "p", "section", "article", "header", "footer", "nav", "aside", "main", "figure", "figcaption", "address", "details", "summary", "fieldset", "dl", "dd", "dt", "center", "ul", "ol", "li", "blockquote", "form", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "body", "html",
];

fn plain_box() -> BoxStyle {
    BoxStyle { margin: [0.0; 4], auto_left: false, auto_right: false, padding: [0.0; 4], border_w: [0.0; 4], border_c: [[0, 0, 0, 255]; 4], radius: 0.0, bg: None, width: Length::Auto, max_width: Length::Auto, min_width: Length::Auto, fx: Fx::default() }
}

fn box_style(c: &Computed, drop_bg: bool) -> BoxStyle {
    BoxStyle {
        margin: c.margin,
        auto_left: c.auto_left,
        auto_right: c.auto_right,
        padding: c.padding,
        border_w: c.border_w,
        border_c: c.border_c,
        radius: c.radius,
        bg: if drop_bg { None } else { c.bg.filter(|x| x[3] > 0) },
        width: c.width,
        max_width: c.max_width,
        min_width: c.min_width,
        fx: c.fx.clone(),
    }
}

fn wrap_box(c: &Computed, tag: &str, children: Vec<Block>) -> Block {
    // the page's background lives on `body`/`html` is the whole view's (see `Doc::canvas`), not a box
    Block::Box(BoxBlock { style: box_style(c, matches!(tag, "body" | "html")), children })
}

pub fn build(tree: &Tree) -> Doc {
    build_with(tree, None, HashMap::new())
}

/// Build with style sheets applied: colours, sizes, hidden elements, boxes, columns.
pub fn build_styled(tree: &Tree, sheets: &StyleSheets, vp: &crate::css::Viewport) -> Doc {
    let computed = style::compute(tree, sheets, vp);
    let pseudo = style::compute_pseudo(tree, sheets, vp, &computed);
    build_with(tree, Some(&computed), pseudo)
}

fn build_with(tree: &Tree, cs: Option<&[Option<Computed>]>, pseudo: HashMap<(usize, PseudoEl), (String, Computed)>) -> Doc {
    let mut b = Builder { tree, cs, pseudo, doc: Doc::default(), cur: None, style: Style::default(), link: None, lists: vec![], quote: 0, text_len: 0, pre: None, form: None, align: Align::Left, transform: Transform::None, col_child: usize::MAX, shadow_hosts: Vec::new() };
    if let Some(t) = tree.find("title") {
        b.doc.title = collapse(&tree.text_of(t)).trim().to_string();
    }
    b.doc.truncated = tree.truncated;
    if let Some(cs) = cs {
        // the page's own background belongs to the whole view: the one on `body`, else the one on `html`
        let bg_of = |tag: &str| tree.find(tag).and_then(|n| cs[n].as_ref()).and_then(|c| c.bg).filter(|c| c[3] > 0);
        b.doc.canvas = bg_of("body").or_else(|| bg_of("html")).map(|c| style::over(c, [255, 255, 255, 255]));
    }
    b.walk(0, 0);
    b.flush();
    let mut doc = b.doc;
    let hide = |n: usize| cs.and_then(|c| c.get(n)).and_then(|c| c.as_ref()).is_some_and(|c| c.display == Display::None);
    let mut tree_a11y = crate::a11y::build(tree, &hide);
    tree_a11y.attach(&doc.links, &doc.controls);
    doc.access = Some(std::sync::Arc::new(tree_a11y));
    doc
}

/// The first address in a `srcset` ("a.png 1x, b.png 2x" gives "a.png").
fn first_srcset(v: &str) -> Option<&str> {
    v.split(',').next().and_then(|c| c.split_whitespace().next()).filter(|u| !u.is_empty())
}

/// Parse and build in one step.
pub fn from_html(src: &str) -> Doc {
    build(&html::parse(src))
}

/// A plain-text page.
pub fn from_text(src: &str) -> Doc {
    Doc { title: String::new(), canvas: None, blocks: vec![Block::Pre(src.chars().take(MAX_TEXT).collect())], links: vec![], controls: vec![], forms: vec![], images: vec![], inlines: vec![], truncated: src.chars().count() > MAX_TEXT, access: None }
}

struct Builder<'a> {
    tree: &'a Tree,
    /// Computed styles, when style sheets are in use.
    cs: Option<&'a [Option<Computed>]>,
    /// Generated `::before` / `::after` content: text and style.
    pseudo: HashMap<(usize, PseudoEl), (String, Computed)>,
    doc: Doc,
    cur: Option<TextBlock>,
    style: Style,
    link: Option<usize>,
    /// For each open list: ordered?, next number.
    lists: Vec<(bool, u32)>,
    quote: u8,
    text_len: usize,
    pre: Option<String>,
    /// The form being read (index into the document's forms).
    form: Option<usize>,
    align: Align,
    transform: Transform,
    /// The element that is a column of a row right now (it is laid out without its list marker).
    col_child: usize,
    /// Elements whose shadow root is being walked (their own children fill its `<slot>`s).
    shadow_hosts: Vec<usize>,
}

fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            if space && out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    if space {
        out.push(' ');
    }
    out
}

impl<'a> Builder<'a> {
    fn full(&self) -> bool {
        self.doc.blocks.len() >= MAX_BLOCKS || self.text_len >= MAX_TEXT
    }

    fn add_span(&mut self, text: String, style: Style) {
        if text.is_empty() {
            return;
        }
        self.text_len += text.len();
        let span = Span { text, style, link: self.link, control: None, image: None, inline: None };
        self.push_span(span);
    }

    /// Put a span (a word, a picture, a control, an inline box) at the end of the text being collected.
    fn push_span(&mut self, span: Span) {
        let quote = self.quote > 0;
        let align = self.align;
        self.cur.get_or_insert_with(|| TextBlock { quote, align, ..TextBlock::default() }).spans.push(span);
    }

    /// The space an inline element's padding (painted with its background) or margin leaves beside its words.
    fn add_gap(&mut self, px: f32, bg: Option<Rgba>) {
        if px < 0.5 || self.pre.is_some() || self.full() {
            return;
        }
        let mut style = self.style;
        style.gap = px.round().clamp(1.0, 2000.0) as i16;
        style.bg = bg;
        self.text_len += 1;
        self.push_span(Span { text: String::new(), style, link: self.link, control: None, image: None, inline: None });
    }

    fn add_text(&mut self, raw: &str) {
        let t = collapse(raw);
        if t.is_empty() {
            return;
        }
        // no space at the start of a line
        let at_start = match &self.cur {
            Some(cur) => cur.spans.iter().rev().find(|s| s.style.gap == 0).is_none_or(|s| s.text.ends_with(' ') || s.text.ends_with('\n')),
            None => true,
        };
        let t = if at_start { t.trim_start_matches(' ').to_string() } else { t };
        let t = match self.transform {
            Transform::None => t,
            Transform::Upper => t.to_uppercase(),
            Transform::Lower => t.to_lowercase(),
            Transform::Capitalize => {
                let mut prev_space = at_start || self.cur.as_ref().is_none_or(|c| c.spans.last().is_none_or(|s| s.text.ends_with(' ')));
                t.chars()
                    .map(|c| {
                        let up = prev_space && c.is_alphabetic();
                        prev_space = c.is_whitespace();
                        if up { c.to_uppercase().next().unwrap_or(c) } else { c }
                    })
                    .collect()
            }
        };
        let style = self.style;
        self.add_span(t, style);
    }

    /// End the current block (or, inside a table cell, start a new line).
    fn flush(&mut self) {
        let Some(mut cur) = self.cur.take() else { return };
        trim_spans(&mut cur.spans);
        // spaces with nothing beside them are no line
        if cur.spans.iter().all(|s| s.style.gap > 0 && s.text.is_empty()) && cur.marker.is_empty() || self.full() {
            return;
        }
        self.doc.blocks.push(Block::Text(cur));
    }

    fn start(&mut self, level: u8, marker: String) {
        self.flush();
        self.cur = Some(TextBlock { level, indent: self.lists.len().saturating_sub(1).min(8) as u8, quote: self.quote > 0, marker, spans: vec![], align: self.align });
    }

    fn walk(&mut self, n: usize, depth: usize) {
        if self.full() || depth > 400 {
            return;
        }
        let tree = self.tree;
        match &tree.nodes[n].kind {
            NodeKind::Text(t) => {
                if let Some(p) = &mut self.pre {
                    p.push_str(t);
                } else {
                    self.add_text(t);
                }
            }
            NodeKind::Element { tag, .. } => {
                if tree.attr(n, "hidden").is_some() || SKIP.contains(&tag.as_str()) {
                    return;
                }
                self.walk_element(n, tag, depth, true);
            }
        }
    }

    fn computed(&self, n: usize) -> Option<&'a Computed> {
        self.cs.and_then(|c| c.get(n)).and_then(|c| c.as_ref())
    }

    /// An element, with what style sheets say about it: hidden, a box, a row of columns, or just different colours.
    fn walk_element(&mut self, n: usize, tag: &str, depth: usize, allow_box: bool) {
        let Some(cs) = self.computed(n) else {
            self.element(n, tag, depth);
            return;
        };
        if cs.display == Display::None || cs.hidden {
            return;
        }
        let saved = (self.style, self.align, self.transform);
        self.style.color = cs.color;
        self.style.underline = cs.underline;
        self.style.strike = cs.strike;
        if let Some(b) = cs.bold {
            self.style.bold = b;
        }
        if let Some(i) = cs.italic {
            self.style.italic = i;
        }
        if cs.mono {
            self.style.code = true;
        }
        self.style.size_pct = if cs.font_set { ((cs.font_px / style::ROOT_PX * 100.0).round() as i32).clamp(30, 600) as u16 } else { 0 };
        // an inline element's own background paints behind its words (links styled as buttons, highlights, badges)
        self.style.bg = if (matches!(tag, "a" | "span" | "b" | "i" | "em" | "strong" | "mark" | "code" | "small" | "label" | "kbd" | "abbr" | "time" | "sup" | "sub" | "button") || cs.display == Display::Inline) && !matches!(cs.display, Display::Block | Display::InlineBlock) { cs.bg.filter(|c| c[3] > 0).or(self.style.bg) } else { None };
        self.align = cs.align;
        self.transform = cs.transform;
        let block_level = BLOCK_TAGS.contains(&tag) && !(cs.display == Display::Inline && tag != "li") || matches!(cs.display, Display::Block | Display::Flex | Display::Grid) || (cs.fx.float != Float::None || cs.fx.pos == style::Pos::Absolute) && !matches!(tag, "img" | "input" | "button" | "select" | "textarea");
        let in_cell = self.pre.is_some();
        let inline_block = cs.display == Display::InlineBlock
            && allow_box
            && !in_cell
            && self.col_child != n
            && cs.fx.float == Float::None
            && cs.fx.pos != style::Pos::Absolute
            && !matches!(tag, "img" | "input" | "button" | "select" | "textarea" | "br" | "table" | "video" | "audio" | "hr")
            && self.doc.inlines.len() < 4000;
        let row = !in_cell && matches!(cs.display, Display::Flex | Display::Grid) && (cs.display == Display::Grid || cs.flex_row) && tag != "table";
        let boxed = !in_cell && allow_box && block_level && tag != "table" && tag != "li" || !in_cell && allow_box && tag == "li" && !cs.display.is_inline() && cs.has_box();
        // the background of `body` and `html` is the whole view's, so it alone makes no box
        let wants_box = if matches!(tag, "body" | "html") { cs.has_box_without_background() } else { cs.has_box() };
        let boxed = boxed && wants_box && tag != "html";
        if inline_block {
            // an atomic box in the running text: its own blocks inside, the text around it carries on after it
            let saved_cur = self.cur.take();
            let saved_blocks = std::mem::take(&mut self.doc.blocks);
            let saved_link = self.link;
            self.element_in(n, tag, depth, cs);
            self.flush();
            let kids = std::mem::replace(&mut self.doc.blocks, saved_blocks);
            self.cur = saved_cur;
            self.link = saved_link;
            if (!kids.is_empty() || cs.has_box() || !matches!(cs.height, Length::Auto)) && !self.full() {
                self.doc.inlines.push(InlineBox { style: box_style(cs, false), height: cs.height, min_height: cs.min_height, valign: cs.valign, children: kids });
                let idx = self.doc.inlines.len() - 1;
                self.text_len += 1;
                let span = Span { text: String::new(), style: self.style, link: None, control: None, image: None, inline: Some(idx) };
                self.push_span(span);
            }
        } else if row {
            self.flush();
            let saved_blocks = std::mem::take(&mut self.doc.blocks);
            self.row_children(n, cs, depth);
            let mut rows = std::mem::replace(&mut self.doc.blocks, saved_blocks);
            // positioned children are out of the flow (they are no columns); the row itself comes last
            let areas = cs.grid_areas.is_some() && cs.display == Display::Grid;
            let row_block = if !areas && matches!(rows.last(), Some(Block::Row(_))) { rows.pop() } else { None };
            if areas && allow_box && wants_box && !rows.is_empty() {
                // a grid laid out by its named areas: rows and full-width pieces, in one box
                self.doc.blocks.push(wrap_box(cs, tag, std::mem::take(&mut rows)));
            }
            self.doc.blocks.extend(rows);
            if let Some(r) = row_block {
                self.doc.blocks.push(if allow_box && wants_box { wrap_box(cs, tag, vec![r]) } else { r });
            }
        } else if boxed {
            self.flush();
            let saved_blocks = std::mem::take(&mut self.doc.blocks);
            self.element_in(n, tag, depth, cs);
            self.flush();
            let kids = std::mem::replace(&mut self.doc.blocks, saved_blocks);
            let keep_empty = cs.fx.float != Float::None || cs.fx.clear != style::Clear::None || matches!(cs.fx.pos, style::Pos::Absolute);
            if (!kids.is_empty() || keep_empty) && !self.full() {
                self.doc.blocks.push(wrap_box(cs, tag, kids));
            }
        } else if cs.display == Display::Block && !BLOCK_TAGS.contains(&tag) && !in_cell {
            // a link or span the page made a block of
            self.flush();
            self.element_in(n, tag, depth, cs);
            self.flush();
        } else if !block_level && !in_cell && !matches!(tag, "img" | "input" | "button" | "select" | "textarea" | "br" | "table" | "td" | "th" | "tr") && cs.display != Display::Block {
            // an inline element: its padding, border and margin leave room beside its words
            let (pl, pr) = (cs.padding[3] + cs.border_w[3], cs.padding[1] + cs.border_w[1]);
            let (ml, mr) = (cs.margin[3].max(0.0), cs.margin[1].max(0.0));
            let bg = self.style.bg;
            self.add_gap(ml, None);
            self.add_gap(pl, bg);
            self.element_in(n, tag, depth, cs);
            self.add_gap(pr, bg);
            self.add_gap(mr, None);
        } else {
            self.element_in(n, tag, depth, cs);
        }
        (self.style, self.align, self.transform) = saved;
    }

    fn element_in(&mut self, n: usize, tag: &str, depth: usize, cs: &Computed) {
        // list markers can be switched off, and a list item the page made inline has none
        if tag == "li" && (cs.display.is_inline() || self.col_child == n) {
            self.children(n, depth);
            return;
        }
        // a block tag the page made `display:inline` (a `div` in a run of text) takes no line of its own
        if cs.display == Display::Inline && matches!(tag, "div" | "p" | "section" | "article" | "header" | "footer" | "nav" | "aside" | "main" | "figure" | "figcaption" | "address" | "center" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "dl" | "dd" | "dt" | "blockquote" | "fieldset") && self.col_child != n {
            self.children(n, depth);
            return;
        }
        if tag == "li" && cs.list_none {
            let saved = std::mem::take(&mut self.lists);
            self.lists.push((false, 1));
            self.start(0, String::new());
            self.children(n, depth);
            self.flush();
            self.lists = saved;
            return;
        }
        self.element(n, tag, depth);
    }

    /// The children of a `display:flex` (row) or grid element, one column each.
    fn row_children(&mut self, n: usize, cs: &Computed, depth: usize) {
        if cs.display == Display::Grid && cs.grid_areas.is_some() && self.area_children(n, cs, depth) {
            return;
        }
        let tree = self.tree;
        let mut cols: Vec<Column> = Vec::new();
        for &c in &tree.nodes[n].children {
            if cols.len() >= 200 || self.full() {
                break;
            }
            let (style, grow, basis, tag) = match &tree.nodes[c].kind {
                NodeKind::Text(t) if t.trim().is_empty() => continue,
                NodeKind::Text(_) => (plain_box(), 0.0, Length::Auto, ""),
                NodeKind::Element { tag, .. } => {
                    if tree.attr(c, "hidden").is_some() || SKIP.contains(&tag.as_str()) {
                        continue;
                    }
                    match self.computed(c) {
                        Some(k) if k.display == Display::None || k.hidden => continue,
                        Some(k) if k.fx.pos == style::Pos::Absolute => {
                            // out of the flow: laid out where it is positioned, not as a column
                            self.flush();
                            self.walk_element(c, tag, depth + 1, true);
                            self.flush();
                            continue;
                        }
                        Some(k) => (box_style(k, false), k.grow, if matches!(k.basis, Length::Auto) { k.width } else { k.basis }, tag.as_str()),
                        None => (plain_box(), 0.0, Length::Auto, tag.as_str()),
                    }
                }
            };
            self.flush();
            let saved_blocks = std::mem::take(&mut self.doc.blocks);
            let saved_col = std::mem::replace(&mut self.col_child, c);
            match &tree.nodes[c].kind {
                NodeKind::Element { .. } => self.walk_element(c, tag, depth + 1, false),
                NodeKind::Text(t) => self.add_text(t),
            }
            self.flush();
            self.col_child = saved_col;
            let blocks = std::mem::replace(&mut self.doc.blocks, saved_blocks);
            if !blocks.is_empty() {
                cols.push(Column { style, grow, basis, blocks });
            }
        }
        if !cols.is_empty() {
            let grid = cs.display == Display::Grid;
            self.doc.blocks.push(Block::Row(RowBlock { style: plain_box(), gap: cs.gap, row_gap: cs.row_gap, justify: cs.justify, wrap: cs.wrap || grid, grid, grid_cols: cs.grid_cols, grid_min: cs.grid_min, cols }));
        }
    }

    /// The children of a grid with `grid-template-areas`: each goes in the column its area starts in (stacked in the
    /// order of their rows); one that spans every column is a block of its own, and the columns around it are rows
    /// of their own. Returns false when the children do not name areas (the plain grid then lays them out).
    fn area_children(&mut self, n: usize, cs: &Computed, depth: usize) -> bool {
        let tree = self.tree;
        let Some(areas) = cs.grid_areas.clone() else { return false };
        let ncols = areas[0].len();
        // where each name sits: first row, first column, how many columns
        let place = |name: &str| -> Option<(usize, usize, usize)> {
            let (mut r0, mut c0, mut c1) = (usize::MAX, usize::MAX, 0);
            for (ri, row) in areas.iter().enumerate() {
                for (ci, a) in row.iter().enumerate() {
                    if a == name {
                        r0 = r0.min(ri);
                        c0 = c0.min(ci);
                        c1 = c1.max(ci + 1);
                    }
                }
            }
            (r0 != usize::MAX).then_some((r0, c0, c1 - c0))
        };
        struct Child {
            node: usize,
            row: usize,
            col: usize,
            span: usize,
        }
        let mut kids: Vec<Child> = Vec::new();
        for &c in &tree.nodes[n].children {
            let NodeKind::Element { tag, .. } = &tree.nodes[c].kind else { continue };
            if tree.attr(c, "hidden").is_some() || SKIP.contains(&tag.as_str()) {
                continue;
            }
            let Some(k) = self.computed(c) else { continue };
            if k.display == Display::None || k.hidden || k.fx.pos == style::Pos::Absolute {
                continue;
            }
            let Some((row, col, span)) = k.grid_area.as_deref().and_then(place) else { continue };
            kids.push(Child { node: c, row, col, span });
        }
        if kids.is_empty() {
            return false;
        }
        kids.sort_by_key(|k| (k.row, k.col));
        let tracks = cs.grid_tracks.clone();
        let mut pending: Vec<Vec<Column>> = (0..ncols).map(|_| Vec::new()).collect();
        let gap = cs.gap;
        macro_rules! flush_row {
            () => {{
                let mut cols: Vec<Column> = Vec::new();
                for (ci, group) in pending.iter_mut().enumerate() {
                    if group.is_empty() {
                        continue;
                    }
                    let mut blocks = Vec::new();
                    for part in group.drain(..) {
                        blocks.extend(part.blocks);
                    }
                    let (grow, basis) = match tracks.as_ref().and_then(|t| t.get(ci)) {
                        Some(style::Track::Px(p)) => (0.0, Length::Px(*p)),
                        Some(style::Track::Fr(f)) => (*f, Length::Px(0.0)),
                        _ => (0.0, Length::Auto),
                    };
                    cols.push(Column { style: plain_box(), grow, basis, blocks });
                }
                if !cols.is_empty() {
                    self.doc.blocks.push(Block::Row(RowBlock { style: plain_box(), gap, row_gap: cs.row_gap, justify: cs.justify, wrap: false, grid: false, grid_cols: 0, grid_min: 0.0, cols }));
                }
            }};
        }
        for k in kids {
            let c = k.node;
            let tag = tree.tag(c).unwrap_or("").to_string();
            self.flush();
            let saved_blocks = std::mem::take(&mut self.doc.blocks);
            self.walk_element(c, &tag, depth + 1, true);
            self.flush();
            let blocks = std::mem::replace(&mut self.doc.blocks, saved_blocks);
            if blocks.is_empty() {
                continue;
            }
            if k.span >= ncols {
                flush_row!();
                self.doc.blocks.extend(blocks);
            } else {
                pending[k.col].push(Column { style: plain_box(), grow: 0.0, basis: Length::Auto, blocks });
            }
        }
        flush_row!();
        true
    }

    /// The text a `::before` / `::after` of element `n` generates, in its own style: inline in the running text, or a
    /// block of its own (with its box) when the page made it `display:block`. Private-use characters (icon fonts we
    /// do not have) are left out, so an icon-only box shows nothing.
    fn generated(&mut self, n: usize, which: PseudoEl) {
        let Some((text, cs)) = self.pseudo.remove(&(n, which)) else { return };
        if cs.display == Display::None || cs.hidden || self.full() {
            return;
        }
        let text: String = text.chars().filter(|c| !matches!(*c, '\u{e000}'..='\u{f8ff}' | '\u{f0000}'..='\u{10ffff}')).map(|c| if c == '\n' { ' ' } else { c }).collect();
        if text.is_empty() {
            return;
        }
        let saved = (self.style, self.align, self.transform);
        self.style.color = cs.color;
        self.style.underline = cs.underline;
        self.style.strike = cs.strike;
        if let Some(b) = cs.bold {
            self.style.bold = b;
        }
        if let Some(i) = cs.italic {
            self.style.italic = i;
        }
        if cs.mono {
            self.style.code = true;
        }
        self.style.size_pct = if cs.font_set { ((cs.font_px / style::ROOT_PX * 100.0).round() as i32).clamp(30, 600) as u16 } else { self.style.size_pct };
        let block = matches!(cs.display, Display::Block | Display::Flex | Display::Grid);
        self.style.bg = if block { None } else { cs.bg.filter(|c| c[3] > 0) };
        self.align = cs.align;
        self.transform = cs.transform;
        let text = match self.transform {
            Transform::Upper => text.to_uppercase(),
            Transform::Lower => text.to_lowercase(),
            _ => text,
        };
        if block && self.pre.is_none() {
            self.flush();
            let saved_blocks = std::mem::take(&mut self.doc.blocks);
            self.start(0, String::new());
            let style = self.style;
            self.add_span(text, style);
            self.flush();
            let kids = std::mem::replace(&mut self.doc.blocks, saved_blocks);
            if cs.has_box() && !kids.is_empty() {
                self.doc.blocks.push(wrap_box(&cs, "div", kids));
            } else {
                self.doc.blocks.extend(kids);
            }
        } else if let Some(p) = &mut self.pre {
            p.push_str(&text);
        } else {
            let style = self.style;
            self.add_span(text, style);
        }
        (self.style, self.align, self.transform) = saved;
    }

    fn children(&mut self, n: usize, depth: usize) {
        if !self.pseudo.is_empty() {
            self.generated(n, PseudoEl::Before);
            self.children_inner(n, depth);
            self.generated(n, PseudoEl::After);
        } else {
            self.children_inner(n, depth);
        }
    }

    fn children_inner(&mut self, n: usize, depth: usize) {
        let tree = self.tree;
        // a shadow root attached by a script (`attachShadow`) is shown instead of the element's own children, which
        // appear where the shadow tree has a `<slot>`
        if let Some(sid) = tree.attr(n, "data-iyakki-shadow").and_then(|v| v.parse::<usize>().ok()).filter(|&i| i < tree.nodes.len()) {
            self.shadow_hosts.push(n);
            for &c in &tree.nodes[sid].children {
                self.walk(c, depth + 1);
            }
            self.shadow_hosts.pop();
            return;
        }
        for &c in &tree.nodes[n].children {
            self.walk(c, depth + 1);
        }
    }

    fn element(&mut self, n: usize, tag: &str, depth: usize) {
        let tree = self.tree;
        match tag {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = tag.as_bytes()[1] - b'0';
                self.start(level, String::new());
                self.children(n, depth);
                self.flush();
            }
            "details" if tree.attr(n, "open").is_none() => {
                // a closed disclosure shows only its summary
                self.flush();
                for &c in &tree.nodes[n].children {
                    if tree.tag(c) == Some("summary") {
                        self.walk(c, depth + 1);
                    }
                }
                self.flush();
            }
            t if BLOCKISH.contains(&t) => {
                self.flush();
                self.children(n, depth);
                self.flush();
            }
            "br" if self.pre.is_none() && tree.attr(n, "clear").is_some_and(|c| matches!(c.trim().to_ascii_lowercase().as_str(), "all" | "both" | "left" | "right")) => {
                // `<br clear=all>`: what follows starts below the floats
                let clear = match tree.attr(n, "clear").unwrap_or("").trim().to_ascii_lowercase().as_str() {
                    "left" => style::Clear::Left,
                    "right" => style::Clear::Right,
                    _ => style::Clear::Both,
                };
                self.flush();
                if !self.full() {
                    let mut st = plain_box();
                    st.fx.clear = clear;
                    self.doc.blocks.push(Block::Box(BoxBlock { style: st, children: Vec::new() }));
                }
            }
            "br" => {
                if self.pre.is_some() {
                    if let Some(p) = &mut self.pre {
                        p.push('\n');
                    }
                } else {
                    let style = Style::default();
                    self.add_span("\n".into(), style);
                }
            }
            "hr" => {
                self.flush();
                if !self.full() {
                    self.doc.blocks.push(Block::Rule);
                }
            }
            "ul" | "ol" => {
                self.flush();
                let start = tree.attr(n, "start").and_then(|s| s.parse().ok()).unwrap_or(1);
                self.lists.push((tag == "ol", start));
                self.children(n, depth);
                self.lists.pop();
                self.flush();
            }
            "li" => {
                let marker = match self.lists.last_mut() {
                    Some((true, k)) => {
                        let m = format!("{k}.");
                        *k += 1;
                        m
                    }
                    _ => "•".to_string(),
                };
                if self.lists.is_empty() {
                    self.lists.push((false, 1));
                    self.start(0, marker);
                    self.children(n, depth);
                    self.flush();
                    self.lists.pop();
                } else {
                    self.start(0, marker);
                    self.children(n, depth);
                    self.flush();
                }
            }
            "dt" => {
                self.start(0, String::new());
                let saved = self.style;
                self.style.bold = true;
                self.children(n, depth);
                self.style = saved;
                self.flush();
            }
            "dd" => {
                self.lists.push((false, 1));
                self.start(0, String::new());
                self.children(n, depth);
                self.flush();
                self.lists.pop();
            }
            "blockquote" => {
                self.flush();
                self.quote = self.quote.saturating_add(1);
                self.children(n, depth);
                self.flush();
                self.quote = self.quote.saturating_sub(1);
            }
            "pre" => {
                self.flush();
                let saved = self.pre.replace(String::new());
                self.children(n, depth);
                let text = std::mem::replace(&mut self.pre, saved).unwrap_or_default();
                let text = text.strip_prefix('\n').unwrap_or(&text).trim_end().to_string();
                if !text.is_empty() && !self.full() {
                    self.text_len += text.len();
                    self.doc.blocks.push(Block::Pre(text));
                } else if !text.is_empty() {
                    let style = Style { code: true, ..Style::default() };
                    self.add_span(text, style);
                }
            }
            "code" | "kbd" | "samp" | "tt" => self.styled(n, depth, |s| s.code = true),
            "b" | "strong" => self.styled(n, depth, |s| s.bold = true),
            "i" | "em" | "cite" | "var" | "dfn" => self.styled(n, depth, |s| s.italic = true),
            "a" => {
                let href = tree.attr(n, "href").map(str::trim).filter(|h| !h.is_empty());
                let saved = self.link;
                if let Some(h) = href {
                    if self.doc.links.len() < 10_000 {
                        self.doc.links.push(h.to_string());
                        self.link = Some(self.doc.links.len() - 1);
                    }
                }
                self.children(n, depth);
                self.link = saved;
            }
            "img" => {
                if self.doc.images.len() < 500 && !self.full() {
                    let src = tree.attr(n, "src").map(str::trim).filter(|v| !v.is_empty()).or_else(|| tree.attr(n, "data-src").map(str::trim).filter(|v| !v.is_empty())).or_else(|| first_srcset(tree.attr(n, "srcset").unwrap_or(""))).unwrap_or("");
                    let (float, fit) = self.computed(n).map_or((Float::None, Fit::Fill), |c| (c.fx.float, c.fx.fit));
                    // a size in pixels from style sheets counts like the attributes
                    let css_px = |l: Length| match l {
                        Length::Px(p) if (1.0..=10_000.0).contains(&p) => Some(p as u32),
                        _ => None,
                    };
                    let (css_w, css_h) = self.computed(n).map_or((None, None), |c| (css_px(c.width), css_px(c.height)));
                    let alt = collapse(tree.attr(n, "alt").unwrap_or("")).trim().to_string();
                    let dim = |k: &str| tree.attr(n, k).and_then(|v| v.trim().trim_end_matches("px").parse::<u32>().ok()).filter(|v| (1..=10_000).contains(v));
                    self.doc.images.push(ImageRef { src: src.to_string(), alt, width: dim("width").or(css_w), height: dim("height").or(css_h), float, fit });
                    let idx = self.doc.images.len() - 1;
                    let span = Span { text: String::new(), style: self.style, link: self.link, control: None, image: Some(idx), inline: None };
                    self.text_len += 1;
                    self.push_span(span);
                }
            }
            "slot" => {
                if let Some(&host) = self.shadow_hosts.last() {
                    let name = tree.attr(n, "name").unwrap_or("");
                    let assigned: Vec<usize> = tree.nodes[host]
                        .children
                        .iter()
                        .copied()
                        .filter(|&c| if tree.tag(c).is_some() { tree.attr(c, "slot").unwrap_or("") == name } else { name.is_empty() })
                        .collect();
                    if !assigned.is_empty() {
                        self.shadow_hosts.pop();
                        for c in assigned {
                            self.walk(c, depth + 1);
                        }
                        self.shadow_hosts.push(host);
                        return;
                    }
                }
                self.children(n, depth);
            }
            "video" | "audio" => {
                // playing is not possible yet: show that there is a video, its poster, and where the file is
                let src = tree.attr(n, "src").map(str::trim).filter(|v| !v.is_empty()).map(str::to_string).or_else(|| {
                    tree.nodes[n].children.iter().find(|&&c| tree.tag(c) == Some("source")).and_then(|&c| tree.attr(c, "src")).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
                });
                self.flush();
                if let Some(poster) = tree.attr(n, "poster").map(str::trim).filter(|v| !v.is_empty()) {
                    if self.doc.images.len() < 500 && !self.full() {
                        self.doc.images.push(ImageRef { src: poster.to_string(), alt: String::new(), width: None, height: None, float: Float::None, fit: Fit::Fill });
                        let idx = self.doc.images.len() - 1;
                        let saved = self.link;
                        if let Some(s) = &src {
                            if self.doc.links.len() < 10_000 {
                                self.doc.links.push(s.clone());
                                self.link = Some(self.doc.links.len() - 1);
                            }
                        }
                        let span = Span { text: String::new(), style: self.style, link: self.link, control: None, image: Some(idx), inline: None };
                        self.text_len += 1;
                        self.push_span(span);
                        self.link = saved;
                        self.flush();
                    }
                }
                let what = if tag == "audio" { "Audio" } else { "Video" };
                let label = tree.attr(n, "aria-label").or_else(|| tree.attr(n, "title")).map(|t| collapse(t).trim().to_string()).filter(|t| !t.is_empty());
                let saved = self.link;
                if let Some(s) = &src {
                    if self.doc.links.len() < 10_000 {
                        self.doc.links.push(s.clone());
                        self.link = Some(self.doc.links.len() - 1);
                    }
                }
                let text = match (&label, &src) {
                    (Some(l), _) => format!("▶ {what}: {l}"),
                    (None, Some(_)) => format!("▶ {what} (open the file)"),
                    (None, None) => format!("▶ {what} (this page plays it with scripts)"),
                };
                let style = Style { dim: self.link.is_none(), ..self.style };
                self.add_span(text, style);
                self.link = saved;
                self.flush();
            }
            "form" => {
                self.flush();
                let saved = self.form;
                self.doc.forms.push(Form { action: tree.attr(n, "action").unwrap_or("").trim().to_string(), post: tree.attr(n, "method").is_some_and(|m| m.eq_ignore_ascii_case("post")), enctype: tree.attr(n, "enctype").unwrap_or("").to_ascii_lowercase() });
                self.form = Some(self.doc.forms.len() - 1);
                self.children(n, depth);
                self.form = saved;
                self.flush();
            }
            "input" => {
                let kind = tree.attr(n, "type").unwrap_or("text").to_ascii_lowercase();
                // a check box or radio button made invisible is the page's own toggle switch (menus, tabs): not shown
                if matches!(kind.as_str(), "checkbox" | "radio") && self.computed(n).is_some_and(|k| k.zero_opacity) {
                    return;
                }
                let value = tree.attr(n, "value").unwrap_or("").to_string();
                let (k, label) = match kind.as_str() {
                    "hidden" => (ControlKind::Hidden, String::new()),
                    "password" => (ControlKind::Password, String::new()),
                    "checkbox" => (ControlKind::Checkbox, String::new()),
                    "radio" => (ControlKind::Radio, String::new()),
                    "submit" | "image" => (ControlKind::Submit, if value.is_empty() { tree.attr(n, "alt").filter(|a| !a.is_empty()).unwrap_or("Submit").to_string() } else { value.clone() }),
                    "button" | "reset" => (ControlKind::Button, if value.is_empty() { "Button".to_string() } else { value.clone() }),
                    _ => (ControlKind::Text, String::new()),
                };
                self.control(n, k, label, value);
            }
            "button" => {
                let kind = tree.attr(n, "type").unwrap_or("submit").to_ascii_lowercase();
                let label = collapse(&tree.text_of(n)).trim().to_string();
                // an icon-only button is named by its `aria-label` or `title`, else shown as a small mark
                let label = if label.is_empty() {
                    tree.attr(n, "value").or_else(|| tree.attr(n, "aria-label")).or_else(|| tree.attr(n, "title")).map(|v| collapse(v).trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| "⋯".to_string())
                } else {
                    label
                };
                let value = tree.attr(n, "value").unwrap_or("").to_string();
                self.control(n, if kind == "submit" { ControlKind::Submit } else { ControlKind::Button }, label, value);
            }
            "textarea" => {
                let text = tree.text_of(n);
                let text = text.strip_prefix('\n').unwrap_or(&text).to_string();
                self.control(n, ControlKind::Textarea, String::new(), text);
            }
            "select" => {
                let mut options = Vec::new();
                let mut todo: Vec<usize> = tree.nodes[n].children.iter().rev().copied().collect();
                while let Some(c) = todo.pop() {
                    match tree.tag(c) {
                        Some("option") => {
                            let label = collapse(&tree.text_of(c)).trim().to_string();
                            let value = tree.attr(c, "value").map_or_else(|| label.clone(), str::to_string);
                            options.push(SelectOption { value, label, selected: tree.attr(c, "selected").is_some() });
                        }
                        Some("optgroup") => todo.extend(tree.nodes[c].children.iter().rev()),
                        _ => {}
                    }
                }
                if !options.is_empty() && !options.iter().any(|o| o.selected) {
                    options[0].selected = true;
                }
                let idx = self.control(n, ControlKind::Select, String::new(), String::new());
                if let Some(i) = idx {
                    self.doc.controls[i].options = options;
                }
            }
            "table" => self.table(n, depth),
            _ => self.children(n, depth),
        }
    }

    /// Add a form control at the current position. Hidden controls are kept for submission but not drawn.
    fn control(&mut self, n: usize, kind: ControlKind, label: String, value: String) -> Option<usize> {
        if self.doc.controls.len() >= 5000 || self.full() {
            return None;
        }
        let t = self.tree;
        let num = |k: &str, d: u32| t.attr(n, k).and_then(|v| v.trim().parse::<u32>().ok()).filter(|v| *v > 0).unwrap_or(d);
        let checked = t.attr(n, "checked").is_some();
        let css = self.computed(n);
        self.doc.controls.push(Control {
            kind,
            form: self.form,
            name: t.attr(n, "name").unwrap_or("").to_string(),
            value: if matches!(kind, ControlKind::Checkbox | ControlKind::Radio) && value.is_empty() { "on".into() } else { value },
            label,
            placeholder: t.attr(n, "placeholder").unwrap_or("").to_string(),
            checked,
            options: Vec::new(),
            size: if kind == ControlKind::Textarea { num("cols", 40) } else { num("size", 20) }.min(200),
            rows: num("rows", 3).min(30),
            disabled: t.attr(n, "disabled").is_some(),
            readonly: t.attr(n, "readonly").is_some(),
            width: css.map_or(Length::Auto, |c| c.width),
            height: css.map_or(Length::Auto, |c| c.height),
            margin: css.map_or([0.0; 4], |c| c.margin),
            valign: css.map_or(VAlign::Middle, |c| c.valign),
            node: n,
        });
        let idx = self.doc.controls.len() - 1;
        if kind != ControlKind::Hidden {
            let style = self.style;
            self.text_len += 1;
            let span = Span { text: String::new(), style, link: None, control: Some(idx), image: None, inline: None };
            self.push_span(span);
        }
        Some(idx)
    }

    fn styled(&mut self, n: usize, depth: usize, f: impl FnOnce(&mut Style)) {
        let saved = self.style;
        f(&mut self.style);
        self.children(n, depth);
        self.style = saved;
    }

    fn table(&mut self, n: usize, depth: usize) {
        self.flush();
        let tree = self.tree;
        let num = |node: usize, k: &str, d: f32| tree.attr(node, k).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok()).filter(|v| *v >= 0.0).unwrap_or(d);
        let table_cs = self.computed(n);
        let pad_attr = num(n, "cellpadding", 1.0).min(40.0);
        let spacing = num(n, "cellspacing", 2.0).min(40.0);
        let border_attr = num(n, "border", 0.0);
        let mut lines = border_attr > 0.0;
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        let mut todo: Vec<usize> = tree.nodes[n].children.iter().rev().copied().collect();
        while let Some(c) = todo.pop() {
            match tree.tag(c) {
                Some("thead" | "tbody" | "tfoot") => todo.extend(tree.nodes[c].children.iter().rev()),
                Some("tr") => {
                    let row_cs = self.computed(c);
                    let row_valign = tree.attr(c, "valign").map(str::trim);
                    let mut cells = Vec::new();
                    for &cell in &tree.nodes[c].children {
                        let Some(ctag) = tree.tag(cell).filter(|t| matches!(*t, "td" | "th")) else { continue };
                        if tree.attr(cell, "hidden").is_some() || self.computed(cell).is_some_and(|k| k.display == Display::None) {
                            continue;
                        }
                        let saved = (self.style, self.align, self.transform);
                        let saved_blocks = std::mem::take(&mut self.doc.blocks);
                        if ctag == "th" {
                            self.style.bold = true;
                            lines = true;
                        }
                        self.walk_element(cell, ctag, depth + 1, false);
                        self.flush();
                        let blocks = std::mem::replace(&mut self.doc.blocks, saved_blocks);
                        (self.style, self.align, self.transform) = saved;
                        let cs = self.computed(cell);
                        let span_of = |k: &str| tree.attr(cell, k).and_then(|v| v.trim().parse::<u32>().ok()).filter(|v| *v >= 1).unwrap_or(1).min(100);
                        let valign = match tree.attr(cell, "valign").map(str::trim).or(row_valign).map(str::to_ascii_lowercase).as_deref() {
                            Some("top") => VAlign::Top,
                            Some("bottom") => VAlign::Bottom,
                            Some(_) => VAlign::Middle,
                            None => cs.map_or(VAlign::Middle, |k| k.valign),
                        };
                        let css_pad = cs.map(|k| k.padding).filter(|p| p.iter().any(|v| *v > 0.0));
                        if cs.is_some_and(|k| k.border_w.iter().any(|w| *w > 0.0)) {
                            lines = true;
                        }
                        cells.push(Cell {
                            blocks,
                            colspan: span_of("colspan"),
                            rowspan: span_of("rowspan"),
                            width: cs.map_or(Length::Auto, |k| k.width),
                            nowrap: tree.attr(cell, "nowrap").is_some() || cs.is_some_and(|k| k.nowrap),
                            valign,
                            pad: css_pad.unwrap_or([pad_attr; 4]),
                            bg: cs.and_then(|k| k.bg).or_else(|| row_cs.and_then(|k| k.bg)).filter(|c| c[3] > 0),
                        });
                    }
                    if !cells.is_empty() && rows.len() < 2000 {
                        rows.push(cells);
                    }
                }
                Some("caption") => {
                    self.start(0, String::new());
                    self.children(c, depth + 1);
                    self.flush();
                }
                _ => {}
            }
        }
        if !rows.is_empty() && !self.full() {
            let align = match tree.attr(n, "align").map(str::to_ascii_lowercase).as_deref() {
                Some("center") => Align::Center,
                Some("right") => Align::Right,
                _ => match table_cs {
                    Some(k) if k.auto_left && k.auto_right => Align::Center,
                    Some(k) if k.auto_left => Align::Right,
                    Some(k) if k.hcenter => Align::Center,
                    _ => Align::Left,
                },
            };
            self.doc.blocks.push(Block::Table(Table { rows, width: table_cs.map_or(Length::Auto, |k| k.width), align, bg: table_cs.and_then(|k| k.bg).filter(|c| c[3] > 0), spacing, lines }));
        }
    }
}

/// Drop leading and trailing whitespace and empty spans from a run.
fn trim_spans(spans: &mut Vec<Span>) {
    let blank = |s: &Span| s.text.trim().is_empty() && s.control.is_none() && s.image.is_none() && s.inline.is_none() && s.style.gap == 0;
    while spans.first().is_some_and(blank) {
        spans.remove(0);
    }
    while spans.last().is_some_and(blank) {
        spans.pop();
    }
    if let Some(f) = spans.first_mut() {
        f.text = f.text.trim_start().to_string();
    }
    if let Some(l) = spans.last_mut() {
        l.text = l.text.trim_end().to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(b: &Block) -> String {
        match b {
            Block::Text(t) => format!("{}{}{}{}", if t.level > 0 { format!("h{} ", t.level) } else { String::new() }, "  ".repeat(t.indent as usize), if t.marker.is_empty() { String::new() } else { format!("{} ", t.marker) }, t.spans.iter().map(|s| s.text.as_str()).collect::<String>()),
            Block::Pre(p) => format!("pre:{p}"),
            Block::Rule => "---".into(),
            Block::Box(_) | Block::Row(_) => "box".into(),
            Block::Table(t) => format!("table:{}", t.rows.iter().map(|r| r.iter().map(Cell::text).collect::<Vec<_>>().join("|")).collect::<Vec<_>>().join(";")),
        }
    }

    fn lines(html: &str) -> Vec<String> {
        from_html(html).blocks.iter().map(text).collect()
    }

    #[test]
    fn videos_and_audio_show_a_poster_and_a_link_to_the_file() {
        let d = from_html("<p>before</p><video poster='/p.jpg' controls><source src='/a.mp4' type='video/mp4'></video><audio src='/s.mp3'></audio><video></video>");
        let t = |i: usize| text(&d.blocks[i]);
        assert_eq!(t(0), "before");
        assert_eq!(d.images.len(), 1);
        assert!(t(2).starts_with("▶ Video") && t(3).starts_with("▶ Audio"), "{:?}", d.blocks.iter().map(text).collect::<Vec<_>>());
        assert_eq!(d.links, vec!["/a.mp4", "/a.mp4", "/s.mp3"], "the poster and the label both lead to the file");
        assert!(t(4).contains("scripts"), "a player without a file says why");
    }

    #[test]
    fn closed_details_show_only_their_summary_and_icon_buttons_get_a_name() {
        let d = from_html("<details><summary>More</summary><p>hidden words</p></details><details open><summary>Open</summary><p>shown words</p></details><button aria-label='Search'></button><button></button>");
        let t: Vec<String> = d.blocks.iter().map(text).collect();
        assert_eq!(&t[..3], &["More".to_string(), "Open".to_string(), "shown words".to_string()]);
        assert_eq!(d.controls[0].label, "Search");
        assert_eq!(d.controls[1].label, "⋯");
    }

    #[test]
    fn headings_paragraphs_and_whitespace() {
        let d = from_html("<html><head><title> My  page </title><style>p{}</style></head><body><h1>Big   title</h1>\n<p>First\n paragraph with <b>bold</b> and <i>italic</i>.</p><p>Second</p></body></html>");
        assert_eq!(d.title, "My page");
        assert_eq!(d.blocks.iter().map(text).collect::<Vec<_>>(), vec!["h1 Big title", "First paragraph with bold and italic.", "Second"]);
        let Block::Text(p) = &d.blocks[1] else { panic!() };
        assert!(p.spans.iter().any(|s| s.text == "bold" && s.style.bold) && p.spans.iter().any(|s| s.text == "italic" && s.style.italic));
    }

    #[test]
    fn lists_are_numbered_and_nested() {
        assert_eq!(lines("<ul><li>a<li>b<ol><li>x<li>y</ol></ul><ol start=5><li>five</ol>"), vec!["• a", "• b", "  1. x", "  2. y", "5. five"]);
    }

    #[test]
    fn links_are_collected_and_attached_to_their_text() {
        let d = from_html(r#"<p>See <a href="/a">the <b>guide</b></a>, <a href="https://x.org/">x</a> and <a>no link</a> <a href="  ">blank</a>.</p>"#);
        assert_eq!(d.links, vec!["/a", "https://x.org/"]);
        let Block::Text(p) = &d.blocks[0] else { panic!() };
        let linked: Vec<(&str, Option<usize>)> = p.spans.iter().map(|s| (s.text.as_str(), s.link)).collect();
        assert!(linked.contains(&("guide", Some(0))) && linked.contains(&("x", Some(1))) && linked.contains(&("no link", None)));
    }

    #[test]
    fn preformatted_text_keeps_its_lines_and_code_is_marked() {
        let d = from_html("<pre>\nfn main() {\n    println!(\"hi\");\n}\n</pre><p>use <code>cargo run</code></p>");
        assert_eq!(text(&d.blocks[0]), "pre:fn main() {\n    println!(\"hi\");\n}");
        let Block::Text(p) = &d.blocks[1] else { panic!() };
        assert!(p.spans.iter().any(|s| s.text == "cargo run" && s.style.code));
    }

    #[test]
    fn tables_become_rows_of_cells() {
        let d = from_html("<table><caption>Sizes</caption><thead><tr><th>Name<th>Size</thead><tbody><tr><td>a<td><b>1</b> MB<tr><td>b<td>2 MB</table>");
        assert_eq!(d.blocks.iter().map(text).collect::<Vec<_>>(), vec!["Sizes", "table:Name|Size;a|1 MB;b|2 MB"]);
        // a table inside a table cell is flattened
        let d = from_html("<table><tr><td><table><tr><td>inner</table></table>");
        assert_eq!(d.blocks.iter().map(text).collect::<Vec<_>>(), vec!["table:inner"]);
    }

    #[test]
    fn images_buttons_quotes_rules_and_hidden_things() {
        assert_eq!(lines("<p>a<img src=x alt='A cat'>b <img src=y></p><hr><blockquote><p>quoted</blockquote><p hidden>secret</p><button>Go</button><input type=submit value=Send><input type=text value=nope>"), vec!["ab ", "---", "quoted", ""]);
        let d = from_html("<p>a<img src=x alt='A  cat' width=64 height=\"32px\">b <img data-src=lazy.png><img srcset=\"s1.png 1x, s2.png 2x\"><a href=/z><img src=in-link.png></a></p>");
        assert_eq!(d.images.iter().map(|i| (i.src.as_str(), i.alt.as_str(), i.width, i.height)).collect::<Vec<_>>(), vec![("x", "A cat", Some(64), Some(32)), ("lazy.png", "", None, None), ("s1.png", "", None, None), ("in-link.png", "", None, None)]);
        let Block::Text(p) = &d.blocks[0] else { panic!() };
        assert_eq!(p.spans.iter().filter_map(|s| s.image).collect::<Vec<_>>(), vec![0, 1, 2, 3], "pictures sit in the text where they are");
        assert_eq!(p.spans.iter().find(|s| s.image == Some(3)).and_then(|s| s.link), Some(0), "a picture inside a link is part of the link");
        let d = from_html("<button>Go</button><input type=submit value=Send><input type=text value=nope>");
        assert_eq!(d.controls.iter().map(|c| (c.kind, c.label.as_str())).collect::<Vec<_>>(), vec![(ControlKind::Submit, "Go"), (ControlKind::Submit, "Send"), (ControlKind::Text, "")], "buttons and fields are controls, not text");
        let d = from_html("<blockquote>q</blockquote>");
        assert!(matches!(&d.blocks[0], Block::Text(t) if t.quote));
    }

    #[test]
    fn line_breaks_scripts_and_text_outside_blocks() {
        assert_eq!(lines("loose text<script>alert(1)</script><p>one<br>two</p>more <noscript>no</noscript>"), vec!["loose text", "one\ntwo", "more"]);
        assert!(lines("<script>x</script><style>y</style>").is_empty());
        assert!(lines("   \n  <p>  </p>  ").is_empty(), "blank pages have no blocks");
    }

    #[test]
    fn definition_lists_and_unknown_tags() {
        assert_eq!(lines("<dl><dt>Term<dd>Meaning</dl><custom-tag>kept <font color=red>text</font></custom-tag>"), vec!["Term", "Meaning", "kept text"]);
    }

    #[test]
    fn plain_text_pages_are_one_preformatted_block() {
        let d = from_text("line 1\nline 2");
        assert_eq!(d.blocks, vec![Block::Pre("line 1\nline 2".into())]);
    }

    #[test]
    fn huge_pages_are_cut_not_crashed() {
        let big = "<p>x</p>".repeat(30_000);
        let d = from_html(&big);
        assert!(d.blocks.len() <= MAX_BLOCKS, "{}", d.blocks.len());
        let deep = "<div>".repeat(3000) + "deep text";
        let d = from_html(&deep);
        assert!(d.blocks.len() <= 1);
    }

    #[test]
    fn forms_and_their_controls_are_read() {
        let d = from_html("<form action=\"/search\" method=GET><p>Find: <input name=q value=\"rust\" placeholder=\"words\" size=30> <select name=lang><option value=en>English<option value=de selected>Deutsch</select></p><input type=hidden name=tok value=abc><label><input type=checkbox name=safe value=1 checked> safe</label><textarea name=note rows=5>\nhello</textarea><input type=submit value=Go><button name=b value=2>Press <b>me</b></button></form><form method=post action=/other><input type=password name=pw></form><input name=loose>");
        assert_eq!(d.forms.len(), 2);
        assert_eq!((d.forms[0].action.as_str(), d.forms[0].post, d.forms[1].post), ("/search", false, true));
        let kinds: Vec<(ControlKind, Option<usize>, &str)> = d.controls.iter().map(|c| (c.kind, c.form, c.name.as_str())).collect();
        assert_eq!(kinds, vec![(ControlKind::Text, Some(0), "q"), (ControlKind::Select, Some(0), "lang"), (ControlKind::Hidden, Some(0), "tok"), (ControlKind::Checkbox, Some(0), "safe"), (ControlKind::Textarea, Some(0), "note"), (ControlKind::Submit, Some(0), ""), (ControlKind::Submit, Some(0), "b"), (ControlKind::Password, Some(1), "pw"), (ControlKind::Text, None, "loose")]);
        let q = &d.controls[0];
        assert_eq!((q.value.as_str(), q.placeholder.as_str(), q.size), ("rust", "words", 30));
        assert_eq!(d.controls[1].options.iter().map(|o| (o.value.as_str(), o.selected)).collect::<Vec<_>>(), vec![("en", false), ("de", true)]);
        assert!(d.controls[3].checked && d.controls[3].value == "1");
        assert_eq!((d.controls[4].value.as_str(), d.controls[4].rows), ("hello", 5), "the newline after <textarea> is dropped");
        assert_eq!((d.controls[5].label.as_str(), d.controls[6].label.as_str(), d.controls[6].value.as_str()), ("Go", "Press me", "2"));
        // controls are in the text, hidden ones are not drawn
        let Block::Text(p) = &d.blocks[0] else { panic!("{:?}", d.blocks) };
        assert_eq!(p.spans.iter().filter_map(|s| s.control).collect::<Vec<_>>(), vec![0, 1]);
        let drawn: Vec<usize> = d.blocks.iter().filter_map(|b| if let Block::Text(t) = b { Some(t.spans.iter().filter_map(|s| s.control)) } else { None }).flatten().collect();
        assert!(!drawn.contains(&2), "the hidden field has no place on the page");
    }

    #[test]
    fn a_checkbox_defaults_to_on_and_a_select_to_its_first_option() {
        let d = from_html("<input type=checkbox name=a><input type=radio name=r value=x><select name=s><option>One<option>Two</select><select></select>");
        assert_eq!(d.controls[0].value, "on");
        assert_eq!(d.controls[1].value, "x");
        assert_eq!(d.controls[2].options.iter().map(|o| (o.value.as_str(), o.selected)).collect::<Vec<_>>(), vec![("One", true), ("Two", false)], "no value attribute: the label is the value");
        assert!(d.controls[3].options.is_empty());
    }

    fn styled(html: &str, css: &str) -> Doc {
        let tree = html::parse(html);
        let vp = crate::css::Viewport::default();
        let mut n = 0;
        let sheet = crate::css::parse_sheet(css, &vp, &mut n);
        build_styled(&tree, &StyleSheets::new(&[sheet]), &vp)
    }

    fn all_text(blocks: &[Block], out: &mut Vec<String>) {
        for b in blocks {
            match b {
                Block::Text(t) => out.push(t.spans.iter().map(|s| s.text.as_str()).collect()),
                Block::Box(x) => all_text(&x.children, out),
                Block::Row(r) => r.cols.iter().for_each(|c| all_text(&c.blocks, out)),
                _ => {}
            }
        }
    }

    fn texts(d: &Doc) -> Vec<String> {
        let mut v = Vec::new();
        all_text(&d.blocks, &mut v);
        v
    }

    #[test]
    fn hidden_elements_are_gone_and_colours_reach_the_words() {
        let d = styled("<p class=h>secret</p><p id=a>shown <span style='color:#ff0000;font-weight:bold'>red</span></p><div style='display:none'>x</div>", ".h{display:none} #a{color:#00ff00;text-decoration:underline}");
        assert_eq!(texts(&d), vec!["shown red"]);
        let Block::Text(p) = &d.blocks[0] else { panic!("{:?}", d.blocks) };
        assert_eq!(p.spans[0].style.color, Some([0, 255, 0, 255]));
        assert_eq!(p.spans[0].style.underline, Some(true));
        let red = p.spans.iter().find(|s| s.text == "red").unwrap();
        assert_eq!((red.style.color, red.style.bold), (Some([255, 0, 0, 255]), true));
    }

    #[test]
    fn without_style_sheets_nothing_changes() {
        let html = "<p style='color:red;display:none'>x</p><div class=a>y</div>";
        let plain = from_html(html);
        assert_eq!(texts(&plain), vec!["x", "y"], "inline styles are not applied by the plain builder");
        assert!(plain.canvas.is_none());
    }

    #[test]
    fn boxes_keep_their_children_and_the_page_background_becomes_the_canvas() {
        let d = styled("<body><div class=card><h2>Title</h2><p>text</p></div><p>after</p></body>", "body{background:#101820;color:#eee} .card{background:#223;padding:12px;margin:8px auto;max-width:600px;border:1px solid #444}");
        assert_eq!(d.canvas, Some([0x10, 0x18, 0x20, 255]));
        let card = d.blocks.iter().find_map(|b| if let Block::Box(x) = b { Some(x) } else { None }).expect("a box");
        assert_eq!(card.style.padding, [12.0; 4]);
        assert!(card.style.auto_left && card.style.auto_right);
        assert_eq!(card.style.max_width, Length::Px(600.0));
        assert_eq!(card.style.bg, Some([0x22, 0x22, 0x33, 255]));
        assert_eq!(card.children.len(), 2);
        assert_eq!(texts(&d), vec!["Title", "text", "after"]);
    }

    #[test]
    fn flex_rows_and_grids_become_columns() {
        let d = styled("<div class=r><div>one</div><div style='flex:2'>two</div><span>three</span></div><ul class=nav><li>a</li><li>b</li></ul>", ".r{display:flex;gap:10px} .nav{display:flex;list-style:none}");
        let Block::Row(r) = &d.blocks[0] else { panic!("{:?}", d.blocks) };
        assert_eq!((r.cols.len(), r.gap, r.grid), (3, 10.0, false));
        assert_eq!(r.cols[1].grow, 2.0);
        let Block::Row(nav) = &d.blocks[1] else { panic!() };
        assert_eq!(nav.cols.len(), 2);
        // list items that are columns carry no bullet
        let Block::Text(t) = &nav.cols[0].blocks[0] else { panic!() };
        assert_eq!(t.marker, "");
        let g = styled("<div class=g><i>1</i><i>2</i><i>3</i><i>4</i></div>", ".g{display:grid;grid-template-columns:repeat(2,1fr)}");
        let Block::Row(r) = &g.blocks[0] else { panic!() };
        assert!(r.grid && r.grid_cols == 2 && r.cols.len() == 4);
    }

    #[test]
    fn list_markers_inline_items_text_transform_and_block_links() {
        let d = styled("<ul class=n><li>a</li><li>b</li></ul><ul><li class=i>x</li><li class=i>y</li></ul><p class=u>make me loud</p><a class=b href=/x>link</a><a href=/y>tail</a>", ".n{list-style:none} .i{display:inline} .u{text-transform:uppercase} a.b{display:block}");
        let marks: Vec<String> = d.blocks.iter().filter_map(|b| if let Block::Text(t) = b { Some(t.marker.clone()) } else { None }).collect();
        assert_eq!(&marks[..2], &["".to_string(), "".to_string()], "list-style:none");
        assert_eq!(texts(&d)[2], "xy", "inline items run together without bullets");
        assert_eq!(texts(&d)[3], "MAKE ME LOUD");
        assert_eq!(&texts(&d)[4..], &["link".to_string(), "tail".to_string()], "the block link is on its own line");
    }

    #[test]
    fn an_inline_background_and_sizes_travel_with_the_spans() {
        let d = styled("<p>a <span class=b>badge</span> <b style='font-size:2em'>big</b></p>", ".b{background:#ff0000;color:white}");
        let Block::Text(p) = &d.blocks[0] else { panic!() };
        let badge = p.spans.iter().find(|s| s.text == "badge").unwrap();
        assert_eq!(badge.style.bg, Some([255, 0, 0, 255]));
        let big = p.spans.iter().find(|s| s.text == "big").unwrap();
        assert_eq!(big.style.size_pct, 200);
        assert!(p.spans[0].style.bg.is_none());
    }

    #[test]
    fn generated_content_before_and_after() {
        let d = styled(
            "<p class=q>quoted</p><a href='/x' title='Tip'>link</a><ul><li class=i>one</li></ul>",
            ".q::before { content: \"\\201C\"; color: #f00; font-weight: bold } .q:after { content: open-quote close-quote \" \\2022 \" } a::after { content: \" (\" attr(title) \")\" } .i::before { content: '\\\"' }",
        );
        assert_eq!(texts(&d), vec!["\u{201c}quoted\u{201c}\u{201d} \u{2022}", "link (Tip)", "\"one"]);
        let Block::Text(b) = &d.blocks[0] else { panic!("{:?}", d.blocks[0]) };
        assert_eq!(b.spans[0].style.color, Some([255, 0, 0, 255]));
        assert!(b.spans[0].style.bold);
        assert_eq!(b.spans.len(), 3, "before + text + after on one line");
    }

    #[test]
    fn generated_content_none_icons_and_clearfix_show_nothing() {
        let d = styled(
            "<div class=cf><i class=ic>x</i><span class=n>y</span></div>",
            ".cf::after { content: \"\"; display: table; clear: both } .ic::before { content: \"\\f101\" } .n::before { content: none } .n::after { content: normal }",
        );
        assert_eq!(texts(&d), vec!["xy"]);
    }

    #[test]
    fn generated_block_content_is_its_own_block() {
        let d = styled("<div class=h>body</div>", ".h::before { content: \"Title\"; display: block; background: #eee; padding: 4px }");
        assert!(matches!(&d.blocks[0], Block::Box(b) if b.style.bg == Some([238, 238, 238, 255])), "{:?}", d.blocks);
        assert_eq!(texts(&d), vec!["Title", "body"]);
    }

    #[test]
    fn inline_blocks_are_boxes_in_the_text_and_inline_padding_leaves_gaps() {
        let d = styled("<p>a <span class=b>box <b>words</b></span> z <a class=l href=/x>link</a></p>", ".b{display:inline-block;padding:2px 8px;background:#ff0000;width:90px} .l{padding:0 5px;margin:0 3px}");
        assert_eq!(d.blocks.len(), 1, "no block of its own: {:?}", d.blocks);
        let Block::Text(t) = &d.blocks[0] else { panic!("{:?}", d.blocks) };
        let at = t.spans.iter().position(|s| s.inline == Some(0)).expect("the box is a span in the line");
        assert!(t.spans[at].text.is_empty() && at > 0 && at + 1 < t.spans.len());
        assert_eq!(d.inlines.len(), 1);
        assert_eq!(d.inlines[0].style.padding, [2.0, 8.0, 2.0, 8.0]);
        assert_eq!(d.inlines[0].style.bg, Some([255, 0, 0, 255]));
        assert_eq!(d.inlines[0].style.width, Length::Px(90.0));
        assert!(matches!(&d.inlines[0].children[0], Block::Text(k) if k.spans.iter().map(|s| s.text.as_str()).collect::<String>() == "box words"));
        let gaps: Vec<i16> = t.spans.iter().filter(|s| s.text.is_empty() && s.style.gap > 0).map(|s| s.style.gap).collect();
        assert_eq!(gaps, vec![3, 5, 5, 3], "margin, padding, words, padding, margin");
        // a `div` made inline stays in the line; one that is positioned absolutely is out of it
        let d = styled("<section>x<div class=i>y</div>z</section>", ".i{display:inline}");
        assert_eq!(d.blocks.iter().filter(|b| matches!(b, Block::Text(_))).count(), 1, "{:?}", d.blocks);
    }

    #[test]
    fn table_cells_keep_their_blocks_widths_alignment_and_spans() {
        let d = styled("<!doctype html><center><table width=80% cellpadding=3 cellspacing=0><tr valign=top><td width=25%>a<td colspan=2 align=right nowrap bgcolor=#ff0000>b<p>c</p><tr><td><table><tr><td>deep</table></table></center>", "");
        let Block::Table(t) = &d.blocks[0] else { panic!("{:?}", d.blocks) };
        assert_eq!((t.width, t.align, t.spacing), (Length::Percent(80.0), Align::Center, 0.0));
        assert_eq!(t.rows.len(), 2);
        let (a, b) = (&t.rows[0][0], &t.rows[0][1]);
        assert_eq!((a.width, a.valign, a.pad), (Length::Percent(25.0), VAlign::Top, [3.0; 4]));
        assert_eq!((b.colspan, b.nowrap, b.bg), (2, true, Some([255, 0, 0, 255])));
        assert!(matches!(&b.blocks[0], Block::Text(k) if k.align == Align::Right), "align=right on the cell");
        assert_eq!(b.blocks.len(), 2, "text and a paragraph are blocks in the cell");
        assert!(matches!(&t.rows[1][0].blocks[0], Block::Table(_)), "a table inside a cell is a table");
    }

    #[test]
    fn a_page_without_a_doctype_does_not_pass_text_align_into_tables_but_a_standards_page_does() {
        let quirk = styled("<center><table><tr><td>x</table></center>", "");
        let Block::Table(t) = &quirk.blocks[0] else { panic!() };
        assert_eq!(t.align, Align::Center, "the centre tag still centres the table");
        assert!(matches!(&t.rows[0][0].blocks[0], Block::Text(k) if k.align == Align::Left), "but not its text");
        let std = styled("<!DOCTYPE html><center><table><tr><td>x</table></center>", "");
        let Block::Table(t) = &std.blocks[0] else { panic!() };
        assert!(matches!(&t.rows[0][0].blocks[0], Block::Text(k) if k.align == Align::Center), "standards mode: inherited");
    }

    #[test]
    fn controls_take_css_sizes_and_pictures_in_centred_text_are_centred() {
        let d = styled("<center><img src=a.png width=10 height=10></center><input class=w name=q><div style=\"text-align:center\"><img src=b.png></div>", ".w{width:300px;height:25px;margin:2px 0;vertical-align:top}");
        assert_eq!(d.controls[0].width, Length::Px(300.0));
        assert_eq!(d.controls[0].height, Length::Px(25.0));
        assert_eq!(d.controls[0].margin, [2.0, 0.0, 2.0, 0.0]);
        assert_eq!(d.controls[0].valign, VAlign::Top);
        let aligns: Vec<Align> = d.blocks.iter().filter_map(|b| if let Block::Text(t) = b { Some(t.align) } else { None }).collect();
        assert_eq!(aligns.first(), Some(&Align::Center), "{:?}", d.blocks);
        assert_eq!(aligns.last(), Some(&Align::Center), "{aligns:?}");
    }
}
