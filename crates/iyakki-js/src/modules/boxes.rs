//! Sizes for scripts: where the boxes of a page would be, estimated from the tree and its computed styles.
//!
//! Scripts ask `getBoundingClientRect`, `offsetWidth`, `clientHeight` and the observers for sizes. The reader's real layout
//! (`iyakki-shell`) runs after the scripts and needs the finished page, so this module makes a quick estimate instead of
//! answering zero: block flow with margins, paddings, borders, widths and `max-width`, auto margins centring a box, inline text
//! wrapped by an average glyph width, inline-blocks and replaced elements (pictures, fields, buttons, canvases, frames), flex
//! rows and columns (basis, grow, shrink, wrap, gap, `justify-content`), grids of equal columns and table rows. Text is measured
//! with a fixed average advance (half an em), so a line breaks near, not exactly at, the place a real font would break it.
//! `position` (relative offsets; absolute and fixed boxes placed against their containing block or the window; sticky counts as in flow),
//! floats (shrink-to-fit, left and right edges, `clear`, text narrowed beside them, kept in the height only of a box that contains them)
//! and `box-sizing` are estimated too, as are percentage heights (against a parent whose height is known; the page itself is as high as the
//! window for `html`), `line-height` from styles, and character widths from a table of a common sans-serif face (`glyphs`). Not done: the
//! face the reader really draws with, and the stacking order of `z-index` is kept only for `elementFromPoint` (see [`LayoutState::hit`]).

use iyakki_web::css::{Length, Viewport};
use iyakki_web::html::{NodeKind, Tree};
use iyakki_web::style::{Clear, Computed, Display, Float, Justify, Pos};

use super::glyphs;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The estimated boxes of a page, by node index; `None` for text, for elements that make no box (`display: none`,
/// `<head>`, scripts) and for nodes that are not in the page.
pub struct Boxes {
    pub rects: Vec<Option<Rect>>,
    /// The `display` value of each element that has a box.
    pub display: Vec<&'static str>,
    /// Height of the whole document.
    pub height: f32,
    /// Width of the whole document: the far edge of the widest box, never less than the window.
    pub width: f32,
}

/// Average advance of a glyph, in ems.
const LINE_EM: f32 = 1.149;
const MAX_DEPTH: usize = 160;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Disp {
    None,
    Block,
    Inline,
    InlineBlock,
    Flex,
    Grid,
    Row,
}

impl Disp {
    fn name(self) -> &'static str {
        match self {
            Disp::None => "none",
            Disp::Block => "block",
            Disp::Inline => "inline",
            Disp::InlineBlock => "inline-block",
            Disp::Flex => "flex",
            Disp::Grid => "grid",
            Disp::Row => "table-row",
        }
    }
}

struct Ctx<'a> {
    fonts: &'a super::webfont::FontSet,
    tree: &'a Tree,
    st: &'a [Option<Computed>],
    vp: &'a Viewport,
    rects: Vec<Option<Rect>>,
    ua_margins: bool,
    /// Boxes taken out of the flow by `position: absolute|fixed`, waiting for their containing block: one list per positioned box
    /// being laid out (the first is the window), each entry the box and where it would have stood in the flow.
    abs: Vec<Vec<(usize, f32, f32)>>,
    /// Floats placed so far (left, right, top, bottom in page coordinates), seen by the text of every box beside them.
    fl: Vec<(f32, f32, f32, f32)>,
    /// The height of the containing block's content when it is known (innermost last): what a percentage height is a share of.
    ph: Vec<Option<f32>>,
    /// The column widths of the tables being laid out (the innermost last): the rows of a table share them.
    cols: Vec<Vec<f32>>,
    /// The top margin of the first in-flow block child of the box just laid out, when it collapses through the box's own top (it has no padding or border there).
    first_mt: Option<f32>,
}

/// Estimate the boxes of the page.
pub fn layout(tree: &Tree, styles: &[Option<Computed>], vp: &Viewport, ua_margins: bool) -> Boxes {
    layout_with_fonts(tree, styles, vp, ua_margins, &super::webfont::FontSet::default())
}

/// [`layout`] with the web fonts the page loaded: text set in a family that has one is measured with that font's advances and kerning.
pub fn layout_with_fonts(tree: &Tree, styles: &[Option<Computed>], vp: &Viewport, ua_margins: bool, fonts: &super::webfont::FontSet) -> Boxes {
    let mut cx = Ctx { fonts, tree, st: styles, vp, rects: vec![None; tree.nodes.len()], ua_margins, abs: vec![Vec::new()], fl: Vec::new(), ph: vec![Some(vp.height)], cols: Vec::new(), first_mt: None };
    let html = tree.nodes[0].children.iter().copied().find(|&c| tree.tag(c) == Some("html"));
    let mut height = 0.0;
    if let Some(h) = html {
        let out = cx.block(h, 0.0, 0.0, vp.width, None, 0);
        height = out.h;
    }
    let pending = std::mem::take(&mut cx.abs[0]);
    cx.place_abs(pending, Rect { x: 0.0, y: 0.0, w: vp.width, h: vp.height }, 0);
    // negative margins, paddings and sizes can leave the estimate with a negative or non-finite number: scripts are told a box of no size
    // instead (found by the layout fuzz test)
    let fix = |v: f32| if v.is_finite() { v } else { 0.0 };
    for r in cx.rects.iter_mut().flatten() {
        *r = Rect { x: fix(r.x), y: fix(r.y), w: fix(r.w).max(0.0), h: fix(r.h).max(0.0) };
    }
    let height = fix(height).max(0.0);
    let display = (0..tree.nodes.len()).map(|n| if cx.rects[n].is_some() { cx.disp(n).name() } else { "none" }).collect();
    let width = cx.rects.iter().flatten().map(|r| r.x + r.w).fold(vp.width, f32::max);
    Boxes { rects: cx.rects, display, height, width: fix(width) }
}

fn is_ws(s: &str) -> bool {
    s.chars().all(char::is_whitespace)
}

/// Characters of text after the collapsing of white space.
fn text_len(s: &str) -> usize {
    let mut n = 0;
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                n += 1;
            }
            prev_space = true;
        } else {
            n += 1;
            prev_space = false;
        }
    }
    n
}

struct Out {
    /// Border-box height.
    h: f32,
    mt: f32,
    mb: f32,
}

/// Where the inline content of a block is: the position in the current line and the lines so far.
struct Run {
    avail: f32,
    x: f32,
    /// Top of the current line, relative to the block's content top.
    y: f32,
    lh: f32,
    started: bool,
}

impl Run {
    fn new(avail: f32) -> Run {
        Run { avail: avail.max(1.0), x: 0.0, y: 0.0, lh: 0.0, started: false }
    }
    /// `<br>`: the line ends even when it holds nothing yet (an empty line is one line high).
    fn newline_keep(&mut self, lh: f32) {
        self.lh = self.lh.max(lh);
        self.y += self.lh;
        self.x = 0.0;
        self.lh = 0.0;
        self.started = true;
    }
    fn newline(&mut self) {
        if self.started {
            self.y += self.lh;
        }
        self.x = 0.0;
        self.lh = 0.0;
        self.started = false;
    }
    /// Text that may not wrap (`white-space: nowrap`): it runs on past the edge of the line.
    fn nowrap_text(&mut self, w: f32, lh: f32) {
        self.started = true;
        self.lh = self.lh.max(lh);
        self.x += w;
    }
    /// A space between words: dropped at the start of a line.
    fn gap(&mut self, w: f32, lh: f32) {
        if self.x > 0.0 {
            self.lh = self.lh.max(lh);
            self.x += w;
        }
    }
    /// A word: put on the next line when it does not fit after what is there (a word wider than the line is broken).
    fn word(&mut self, w: f32, lh: f32) {
        if self.x > 0.0 && self.x + w > self.avail + 0.01 {
            self.y += self.lh;
            self.x = 0.0;
            self.lh = 0.0;
        }
        self.text(w, lh);
    }
    /// Put text `w` wide in the line, breaking it over lines when it does not fit. Returns the position after it.
    fn text(&mut self, w: f32, lh: f32) {
        self.started = true;
        self.lh = self.lh.max(lh);
        let mut left = w;
        while self.x + left > self.avail + 0.01 {
            let fit = (self.avail - self.x).max(0.0);
            left -= fit;
            if fit <= 0.0 && self.x == 0.0 {
                break;
            }
            self.y += self.lh;
            self.x = 0.0;
            self.lh = lh;
        }
        self.x += left;
    }
    fn atom(&mut self, w: f32, h: f32) -> (f32, f32) {
        if self.x > 0.0 && self.x + w > self.avail + 0.01 {
            self.newline();
        }
        self.started = true;
        let at = (self.x, self.y);
        self.x += w;
        self.lh = self.lh.max(h);
        at
    }
    fn height(&self) -> f32 {
        if self.started { self.y + self.lh } else { self.y }
    }
}

impl<'a> Ctx<'a> {
    fn style(&self, n: usize) -> Option<&'a Computed> {
        self.st.get(n).and_then(|s| s.as_ref())
    }

    fn disp(&self, n: usize) -> Disp {
        let tag = self.tree.tag(n).unwrap_or("");
        if matches!(tag, "head" | "script" | "style" | "meta" | "link" | "title" | "template" | "base" | "noscript" | "datalist" | "source" | "track" | "param" | "area" | "map") {
            return Disp::None;
        }
        if tag == "input" && self.tree.attr(n, "type").is_some_and(|t| t.eq_ignore_ascii_case("hidden")) {
            return Disp::None;
        }
        if tag == "dialog" && self.tree.attr(n, "open").is_none() {
            return Disp::None;
        }
        if self.tree.attr(n, "hidden").is_some() && self.style(n).is_none_or(|c| c.display == Display::Default) {
            return Disp::None;
        }
        let by_tag = match tag {
            "html" | "body" | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "li" | "dl" | "dt" | "dd" | "section" | "article" | "aside" | "header" | "footer" | "nav" | "main" | "form" | "fieldset" | "figure" | "figcaption" | "address" | "blockquote" | "pre" | "hr" | "table" | "thead" | "tbody" | "tfoot" | "caption" | "details" | "summary" | "dialog" | "center" | "menu" | "legend" | "td" | "th" | "colgroup" => Disp::Block,
            "tr" => Disp::Row,
            "img" | "input" | "button" | "select" | "textarea" | "video" | "audio" | "canvas" | "svg" | "iframe" | "object" | "embed" | "meter" | "progress" => Disp::InlineBlock,
            _ => Disp::Inline,
        };
        match self.style(n).map(|c| c.display) {
            Some(Display::None) => Disp::None,
            Some(Display::Block) => Disp::Block,
            Some(Display::Inline) => if is_replaced(tag) { Disp::InlineBlock } else { Disp::Inline },
            Some(Display::InlineBlock) => Disp::InlineBlock,
            Some(Display::Flex) => Disp::Flex,
            Some(Display::Grid) => Disp::Grid,
            _ => by_tag,
        }
    }

    /// The children that are drawn: for an element with a shadow root (`attachShadow`), the root's content stands in for them.
    fn kids_of(&self, n: usize) -> Vec<usize> {
        if let Some(sid) = self.tree.attr(n, "data-iyakki-shadow").and_then(|v| v.parse::<usize>().ok()).filter(|&i| i < self.tree.nodes.len()) {
            let k = self.tree.nodes[sid].children.clone();
            if !k.is_empty() {
                return k;
            }
        }
        self.tree.nodes[n].children.clone()
    }

    /// The width left on the line at page height `y` of a box `w` wide at `x`, after the floats that reach there.
    fn narrowed(&self, x: f32, w: f32, y: f32) -> f32 {
        let taken: f32 = self.fl.iter().filter(|f| f.3 > y && f.2 <= y).map(|f| (f.1.min(x + w) - f.0.max(x)).max(0.0)).sum();
        (w - taken).max(w * 0.25)
    }

    fn font(&self, n: usize) -> f32 {
        let c = self.style(n);
        let tag = self.tree.tag(n).unwrap_or("");
        match c {
            Some(c) if c.font_set => c.font_px,
            Some(c) => match tag {
                "h1" => 32.0,
                "h2" => 24.0,
                "h3" => 18.72,
                "h5" => 13.28,
                "h6" => 10.72,
                _ => c.font_px,
            },
            None => 16.0,
        }
    }

    /// The width of `text` set in the font of element `n`.
    fn text_w(&self, n: usize, text: &str) -> f32 {
        let c = self.style(n);
        let web = c.and_then(|c| self.fonts.get(&c.family));
        let mut w = match web {
            Some(f) => f.text_em(text),
            None => glyphs::text_face(text, c.is_some_and(|c| c.mono), c.is_none_or(|c| c.serif)),
        } * self.font(n);
        if c.is_some_and(|c| c.bold == Some(true)) || matches!(self.tree.tag(n), Some("h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "b" | "strong" | "th")) {
            w *= 1.06;
        }
        w
    }

    /// The height of a line of text in element `n`: its `line-height`, or 1.15 times the font size (what the common faces use).
    fn line_h(&self, n: usize) -> f32 {
        // a browser rounds the face's metrics to whole pixels: 16 px gives 18, 20 px gives 23 (Arial, Times)
        match self.style(n) {
            Some(c) if c.line_height != iyakki_web::style::LineHeight::Normal => c.line_height.px(self.font(n), LINE_EM),
            _ => (self.font(n) * LINE_EM).round(),
        }
    }

    /// The width of the text below `n` on one line (no wrapping), for shrink-to-fit boxes.
    fn extent(&self, n: usize, depth: usize) -> f32 {
        if depth > 40 {
            return 0.0;
        }
        let mut cur = 0.0f32;
        let mut best = 0.0f32;
        for ch in self.kids_of(n) {
            match &self.tree.nodes[ch].kind {
                NodeKind::Text(t) => cur += self.text_w(n, t),
                NodeKind::Element { .. } => {
                    match self.disp(ch) {
                        Disp::None => {}
                        Disp::Inline => cur += self.extent(ch, depth + 1),
                        Disp::InlineBlock => {
                            let (w, _) = self.replaced(ch).unwrap_or((self.extent(ch, depth + 1), 0.0));
                            cur += w;
                        }
                        _ => {
                            best = best.max(cur);
                            cur = 0.0;
                            best = best.max(self.extent(ch, depth + 1));
                        }
                    }
                }
            }
        }
        best.max(cur)
    }

    /// Size of a replaced element or form control (border box).
    fn replaced(&self, n: usize) -> Option<(f32, f32)> {
        let tag = self.tree.tag(n)?;
        let c = self.style(n);
        let attr_px = |k: &str| self.tree.attr(n, k).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok());
        let css_w = c.and_then(|c| c.width.resolve(self.vp.width, 1.0));
        let css_h = c.and_then(|c| c.height.resolve(self.vp.height, 1.0));
        let (dw, dh): (f32, f32) = match tag {
            "img" => (0.0, 0.0),
            "canvas" | "video" | "iframe" | "object" | "embed" | "svg" => (300.0, 150.0),
            "audio" => (300.0, 54.0),
            "meter" => (80.0, 16.0),
            "progress" => (160.0, 16.0),
            "textarea" => (self.tree.attr(n, "cols").and_then(|v| v.parse::<f32>().ok()).unwrap_or(20.0) * 8.0 + 6.0, self.tree.attr(n, "rows").and_then(|v| v.parse::<f32>().ok()).unwrap_or(2.0) * 15.0 + 6.0),
            "select" => (self.extent(n, 0).max(40.0) + 24.0, 21.0),
            "button" => (self.extent(n, 0) + 16.0, 21.0),
            "input" => {
                let ty = self.tree.attr(n, "type").unwrap_or("text").to_ascii_lowercase();
                match ty.as_str() {
                    "checkbox" | "radio" => (13.0, 13.0),
                    "submit" | "button" | "reset" => (text_len(self.tree.attr(n, "value").unwrap_or("Submit")) as f32 * 8.0 + 16.0, 21.0),
                    "range" => (129.0, 16.0),
                    "image" => (0.0, 0.0),
                    _ => (self.tree.attr(n, "size").and_then(|v| v.parse::<f32>().ok()).unwrap_or(20.0) * 7.0 + 6.0, 21.0),
                }
            }
            _ => return None,
        };
        let w = css_w.or_else(|| attr_px("width")).unwrap_or(dw);
        let h = css_h.or_else(|| attr_px("height")).unwrap_or(dh);
        Some((w, h))
    }

    /// One block-level box. `forced` is a border-box width the parent has decided (flex items, cells).
    fn block(&mut self, n: usize, x: f32, y: f32, avail: f32, forced: Option<f32>, depth: usize) -> Out {
        let d = self.disp(n);
        if d == Disp::None || depth > MAX_DEPTH {
            return Out { h: 0.0, mt: 0.0, mb: 0.0 };
        }
        let c = self.style(n).cloned().unwrap_or_default();
        let tag = self.tree.tag(n).unwrap_or("");
        let (mut mt, mut mr, mut mb, mut ml) = (c.margin[0], c.margin[1], c.margin[2], c.margin[3]);
        let mut pad = c.padding;
        // a percentage margin or padding is a share of the width of the containing block, on every side
        for i in 0..4 {
            if let Some(p) = c.pad_pct[i] {
                pad[i] = (avail * p / 100.0).max(0.0);
            }
        }
        for (i, v) in [&mut mt, &mut mr, &mut mb, &mut ml].into_iter().enumerate() {
            if let Some(p) = c.margin_pct[i] {
                *v = avail * p / 100.0;
            }
        }
        let em = self.font(n);
        if self.ua_margins && c.margin == [0.0; 4] {
            match tag {
                "body" => (mt, mr, mb, ml) = (8.0, 8.0, 8.0, 8.0),
                "p" | "blockquote" | "ul" | "ol" | "dl" | "pre" | "figure" => (mt, mb) = (em, em),
                "h1" => (mt, mb) = (em * 0.67, em * 0.67),
                "h2" => (mt, mb) = (em * 0.83, em * 0.83),
                "h3" => (mt, mb) = (em, em),
                "h4" => (mt, mb) = (em * 1.33, em * 1.33),
                "h5" => (mt, mb) = (em * 1.67, em * 1.67),
                "h6" => (mt, mb) = (em * 2.33, em * 2.33),
                _ => {}
            }
            if matches!(tag, "ul" | "ol") && pad == [0.0; 4] {
                pad[3] = 40.0;
            }
        }
        let bw = c.border_w;
        let frame_h = pad[1] + pad[3] + bw[1] + bw[3];
        let frame_v = pad[0] + pad[2] + bw[0] + bw[2];
        // the border-box width
        let replaced = if d == Disp::InlineBlock { self.replaced(n) } else { None };
        let mut outer_w = match (forced, replaced) {
            (Some(f), _) => f,
            (None, Some((w, _))) => w,
            (None, None) => match c.width.resolve(avail, 1.0) {
                Some(w) => if c.border_box { w } else { w + frame_h },
                None if d == Disp::InlineBlock || c.fx.float != Float::None => (if c.nowrap { self.extent(n, 0) + frame_h + ml + mr } else { (self.extent(n, 0) + frame_h + ml + mr).min(avail) }) - ml - mr,
                None => avail - ml - mr,
            },
        };
        if forced.is_none() {
            if let Some(m) = c.max_width.resolve(avail, 1.0) {
                outer_w = outer_w.min(if c.border_box { m } else { m + frame_h });
            }
            if let Some(m) = c.min_width.resolve(avail, 1.0) {
                outer_w = outer_w.max(if c.border_box { m } else { m + frame_h });
            }
        }
        outer_w = outer_w.max(frame_h).max(0.0);
        // auto margins centre the box
        let free = (avail - outer_w - ml - mr).max(0.0);
        let left = match (c.auto_left, c.auto_right) {
            (true, true) => ml + free / 2.0,
            (true, false) => ml + free,
            _ => ml,
        };
        let bx = x + left;
        let by = y + mt;
        let content_w = (outer_w - frame_h).max(0.0);
        let cx0 = bx + bw[3] + pad[3];
        let cy0 = by + bw[0] + pad[0];
        // a positioned box is the containing block of the absolute boxes below it
        let positioned = c.fx.pos != Pos::Static;
        if positioned {
            self.abs.push(Vec::new());
        }
        // boxes that contain their floats: scrolling boxes, floats, absolute boxes, inline-blocks, the page itself
        let bfc = c.overflow_hidden || c.fx.float != Float::None || matches!(c.fx.pos, Pos::Absolute | Pos::Fixed) || d == Disp::InlineBlock || forced.is_some() || tag == "html";
        // the height this box has before its content is looked at: a length, or a share of a parent whose height is known
        let base = self.ph.last().copied().flatten();
        let def_h = match c.height {
            Length::Auto => None,
            Length::Percent(_) => base.and_then(|b| c.height.resolve(b, 1.0)),
            Length::Calc { .. } | Length::Pick { .. } => c.height.resolve(base.unwrap_or(0.0), 1.0),
            l => l.resolve(0.0, 1.0),
        }
        .filter(|h| h.is_finite());
        let def_content = def_h.map(|h| if c.border_box { (h - frame_v).max(0.0) } else { h });
        self.ph.push(if tag == "html" && def_content.is_none() { None } else { def_content });
        let mut first_mt: Option<f32> = None;
        let content_h = if let Some((_, h)) = replaced {
            (h - frame_v).max(0.0)
        } else {
            if tag == "table" {
                let cols = self.table_columns(n, content_w);
                self.cols.push(cols);
            }
            match d {
                Disp::Flex if c.flex_row => self.flex_row(n, &c, cx0, cy0, content_w, depth),
                Disp::Grid => self.grid(n, &c, cx0, cy0, content_w, depth),
                Disp::Row => self.table_row(n, cx0, cy0, content_w, depth),
                _ => {
                    // a block with no padding or border on top lets its first block child's top margin through (and so does a table's body)
                    self.first_mt = None;
                    let through = !bfc && d == Disp::Block && pad[0] == 0.0 && bw[0] == 0.0;
                    let h = self.flow_with(n, &c, cx0, cy0, content_w, depth, bfc, through);
                    first_mt = self.first_mt.take();
                    h
                }
            }
        };
        self.ph.pop();
        if tag == "table" {
            self.cols.pop();
        }
        let mut h_content = content_h;
        if let Some(h) = def_content {
            h_content = h;
        }
        if let Some(m) = c.max_height.resolve(base.unwrap_or(self.vp.height), 1.0) {
            h_content = h_content.min(if c.border_box { (m - frame_v).max(0.0) } else { m });
        }
        if let Some(m) = c.min_height.resolve(base.unwrap_or(self.vp.height), 1.0) {
            h_content = h_content.max(if c.border_box { (m - frame_v).max(0.0) } else { m });
        }
        let h = h_content + frame_v;
        self.rects[n] = Some(Rect { x: bx, y: by, w: outer_w, h });
        if positioned {
            let pending = self.abs.pop().unwrap_or_default();
            let cb = Rect { x: bx + bw[3], y: by + bw[0], w: (outer_w - bw[1] - bw[3]).max(0.0), h: (h - bw[0] - bw[2]).max(0.0) };
            self.place_abs(pending, cb, depth);
        }
        if c.fx.pos == Pos::Relative {
            let [t, r, b, l] = c.fx.off;
            let pc = c.fx.off_pct;
            let (l, r) = (l.or(pc[3].map(|p| avail * p / 100.0)), r.or(pc[1].map(|p| avail * p / 100.0)));
            let (t, b) = (t.or(pc[0].and_then(|p| base.map(|h| h * p / 100.0))), b.or(pc[2].and_then(|p| base.map(|h| h * p / 100.0))));
            self.shift(n, l.or(r.map(|v| -v)).unwrap_or(0.0), t.or(b.map(|v| -v)).unwrap_or(0.0));
        }
        // the first child's top margin that collapsed through this box's top: the box starts where the larger of the two margins ends
        let mt_out = match first_mt {
            Some(f) if f > mt => {
                self.shift(n, 0.0, f - mt);
                f
            }
            _ => mt,
        };
        Out { h, mt: mt_out, mb }
    }

    /// Children in normal flow: blocks stacked, inline content in lines. Returns the content height.
    #[allow(unused_assignments)]
    #[allow(clippy::too_many_arguments)]
    fn flow_with(&mut self, n: usize, c: &Computed, x: f32, y: f32, w: f32, depth: usize, bfc: bool, through: bool) -> f32 {
        let kids: Vec<usize> = self.kids_of(n);
        let gap = if c.flex_row { 0.0 } else { c.row_gap.max(c.gap) };
        let mut cursor = 0.0f32;
        let mut prev_mb: Option<f32> = None;
        let mut floats: Vec<(Float, f32, f32)> = Vec::new();
        let fl_mark = self.fl.len();
        let mut run = Run::new(w);
        let mut inline_started = false;
        let mut first_block_seen = false;
        let mut first_collapsed: Option<f32> = None;
        macro_rules! flush {
            () => {
                if inline_started {
                    let hh = run.height();
                    cursor += hh;
                    run = Run::new(self.narrowed(x, w, y + cursor));
                    inline_started = false;
                    prev_mb = None;
                }
            };
        }
        for ch in kids {
            match &self.tree.nodes[ch].kind {
                NodeKind::Text(t) => {
                    if is_ws(t) && !inline_started {
                        continue;
                    }
                    let lh = self.line_h(n);
                    if !inline_started {
                        run = Run::new(self.narrowed(x, w, y + cursor));
                        inline_started = true;
                        run.y = 0.0;
                    }
                    self.place_text(n, t, lh, &mut run);
                }
                NodeKind::Element { .. } => {
                    let d = self.disp(ch);
                    if d == Disp::None {
                        continue;
                    }
                    let (pos, fl, clr) = self.style(ch).map_or((Pos::Static, Float::None, Clear::None), |c| (c.fx.pos, c.fx.float, c.fx.clear));
                    if matches!(pos, Pos::Absolute | Pos::Fixed) {
                        // out of the flow: placed when its containing block is known
                        let (sx, sy) = (x + if inline_started { run.x } else { 0.0 }, y + cursor + if inline_started { run.y } else { 0.0 });
                        let frame = if pos == Pos::Fixed { 0 } else { self.abs.len() - 1 };
                        self.abs[frame].push((ch, sx, sy));
                        continue;
                    }
                    if fl != Float::None {
                        let cs = self.style(ch).cloned().unwrap_or_default();
                        let cur_y = cursor + if inline_started { run.y } else { 0.0 };
                        floats.retain(|f| f.2 > cur_y);
                        let probe = self.block(ch, 0.0, 0.0, w, None, depth + 1);
                        let r = self.rects[ch].unwrap_or_default();
                        let ow = r.w + cs.margin[1] + cs.margin[3];
                        let used = |side: Float| floats.iter().filter(|f| f.0 == side).map(|f| f.1).sum::<f32>();
                        let at = if fl == Float::Left { x + used(Float::Left) } else { x + w - used(Float::Right) - ow };
                        self.shift(ch, at + cs.margin[3] - r.x, y + cur_y + cs.margin[0] - r.y);
                        floats.push((fl, ow, cur_y + probe.h + cs.margin[0] + cs.margin[2]));
                        self.fl.push((at, at + ow, y + cur_y, y + cur_y + probe.h + cs.margin[0] + cs.margin[2]));
                        continue;
                    }
                    match d {
                    Disp::None => {}
                    Disp::Inline => {
                        if !inline_started {
                            run = Run::new(self.narrowed(x, w, y + cursor));
                            inline_started = true;
                        }
                        self.inline(ch, x, y + cursor, &mut run, depth + 1);
                    }
                    Disp::InlineBlock => {
                        if !inline_started {
                            run = Run::new(self.narrowed(x, w, y + cursor));
                            inline_started = true;
                        }
                        let cs = self.style(ch).cloned().unwrap_or_default();
                        // size it first (width and height), then drop it in the line
                        let probe = self.block(ch, 0.0, 0.0, w, None, depth + 1);
                        let r = self.rects[ch].unwrap_or_default();
                        let (aw, ah) = (r.w + cs.margin[1] + cs.margin[3], probe.h + cs.margin[0] + cs.margin[2]);
                        let (px, py) = run.atom(aw, ah);
                        // move the box (and everything below it) to where the line put it
                        self.shift(ch, x + px + cs.margin[3] - r.x, y + cursor + py + cs.margin[0] - r.y);
                    }
                    _ => {
                        flush!();
                        if clr != Clear::None {
                            let below = floats.iter().filter(|f| match clr { Clear::Left => f.0 == Float::Left, Clear::Right => f.0 == Float::Right, _ => true }).map(|f| f.2).fold(0.0, f32::max);
                            cursor = cursor.max(below);
                        }
                        // `block` puts the box after its own top margin; margins of neighbours collapse to the larger one
                        let probe = self.block(ch, x, y + cursor, w, None, depth + 1);
                        let between = if cursor > 0.0 && prev_mb.is_some() { gap } else { 0.0 };
                        // a box with nothing in it (no height, padding or border) has its top and bottom margin collapse into one, which also
                        // collapses with the margins around it: it takes its place where the margins so far end and adds only to the pool
                        let empty_box = probe.h == 0.0 && !framed_or_flex(&self.style(ch).cloned().unwrap_or_default(), d);
                        if empty_box {
                            let at = prev_mb.map_or(probe.mt, |p| p.max(probe.mt));
                            self.shift(ch, 0.0, at - probe.mt);
                            prev_mb = Some(prev_mb.unwrap_or(0.0).max(probe.mt).max(probe.mb));
                            if through && !first_block_seen && cursor == 0.0 && !inline_started {
                                // the margins of an empty first child join the box's own top margin
                                first_collapsed = Some(first_collapsed.unwrap_or(0.0).max(probe.mt.max(probe.mb)));
                            }
                            continue;
                        }
                        let mut top = prev_mb.map_or(probe.mt, |p| p.max(probe.mt));
                        // the first child of a box that lets it through: its margin is the box's, the child starts at the box's top edge
                        if through && !first_block_seen && cursor == 0.0 && !inline_started && prev_mb.is_none() {
                            first_collapsed = Some(probe.mt);
                            top = 0.0;
                        }
                        first_block_seen = true;
                        let delta = top + between - probe.mt;
                        self.shift(ch, 0.0, delta);
                        cursor += top + between + probe.h;
                        prev_mb = Some(probe.mb);
                    }
                    }
                }
            }
        }
        flush!();
        if bfc {
            cursor = cursor.max(floats.iter().map(|f| f.2).fold(0.0, f32::max));
        }
        self.fl.truncate(fl_mark);
        // the last child's bottom margin stays inside a box with padding or a border and collapses out of one without
        let framed = c.padding[2] > 0.0 || c.border_w[2] > 0.0;
        if framed {
            cursor += prev_mb.unwrap_or(0.0);
        }
        // handed to `block`, which asked for it: set last, because the boxes laid out below this one cleared it on their own way out
        self.first_mt = first_collapsed;
        cursor
    }

    /// An inline element: its text and children go into the run; its own box is the area they cover.
    /// Text of element `n` in the line, word by word.
    fn place_text(&self, n: usize, t: &str, lh: f32, run: &mut Run) {
        if self.style(n).is_some_and(|c| c.nowrap) {
            run.nowrap_text(self.text_w(n, t), lh);
            return;
        }
        let space = self.text_w(n, " ");
        let words: Vec<&str> = t.split_whitespace().collect();
        if words.is_empty() {
            run.text(self.text_w(n, t), lh);
            return;
        }
        if t.starts_with(char::is_whitespace) {
            run.gap(space, lh);
        }
        for (i, w) in words.iter().enumerate() {
            if i > 0 {
                run.gap(space, lh);
            }
            run.word(self.text_w(n, w), lh);
        }
        if t.ends_with(char::is_whitespace) {
            run.gap(space, lh);
        }
    }

    fn inline(&mut self, n: usize, x: f32, y: f32, run: &mut Run, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        if self.tree.tag(n) == Some("br") {
            let lh = self.line_h(n);
            run.newline_keep(lh);
            return;
        }
        let start = (run.x, run.y, run.started);
        let lh = self.line_h(n);
        let c = self.style(n).cloned().unwrap_or_default();
        run.x += c.margin[3] + c.padding[3] + c.border_w[3];
        let kids: Vec<usize> = self.kids_of(n);
        for ch in kids {
            match &self.tree.nodes[ch].kind {
                NodeKind::Text(t) => {
                    if is_ws(t) && !run.started {
                        continue;
                    }
                    self.place_text(n, t, lh, run);
                }
                NodeKind::Element { .. } => match self.disp(ch) {
                    Disp::None => {}
                    Disp::Inline if self.tree.tag(ch) == Some("br") => run.newline_keep(lh),
                    Disp::Inline => self.inline(ch, x, y, run, depth + 1),
                    _ => {
                        let cs = self.style(ch).cloned().unwrap_or_default();
                        let probe = self.block(ch, 0.0, 0.0, run.avail, None, depth + 1);
                        let r = self.rects[ch].unwrap_or_default();
                        let (px, py) = run.atom(r.w + cs.margin[1] + cs.margin[3], probe.h + cs.margin[0] + cs.margin[2]);
                        self.shift(ch, x + px + cs.margin[3] - r.x, y + py + cs.margin[0] - r.y);
                    }
                },
            }
        }
        run.x += c.margin[1] + c.padding[1] + c.border_w[1];
        let (sx, sy) = if start.2 { (start.0, start.1) } else { (0.0, run.y) };
        let multi_line = run.y > sy + 0.01;
        let rect = if multi_line {
            Rect { x, y: y + sy, w: run.avail, h: run.y + run.lh - sy }
        } else {
            Rect { x: x + sx, y: y + sy, w: (run.x - sx).max(0.0), h: run.lh.max(lh) }
        };
        self.rects[n] = Some(rect);
    }

    /// Put the absolute and fixed boxes that were waiting for a containing block `cb` (its padding box) where their offsets say.
    fn place_abs(&mut self, pending: Vec<(usize, f32, f32)>, cb: Rect, depth: usize) {
        for (ch, sx, sy) in pending {
            let Some(c) = self.style(ch).cloned() else { continue };
            // a fixed box is placed against the window, whichever block it was found in
            let cb = if c.fx.pos == Pos::Fixed { Rect { x: 0.0, y: 0.0, w: self.vp.width, h: self.vp.height } } else { cb };
            let [t, r, b, l] = c.fx.off;
            let pc = c.fx.off_pct;
            let (l, r) = (l.or(pc[3].map(|p| cb.w * p / 100.0)), r.or(pc[1].map(|p| cb.w * p / 100.0)));
            let (t, b) = (t.or(pc[0].map(|p| cb.h * p / 100.0)), b.or(pc[2].map(|p| cb.h * p / 100.0)));
            let frame_h = c.padding[1] + c.padding[3] + c.border_w[1] + c.border_w[3];
            let frame_v = c.padding[0] + c.padding[2] + c.border_w[0] + c.border_w[2];
            let ow = match c.width.resolve(cb.w, 1.0) {
                Some(w) => if c.border_box { w } else { w + frame_h },
                None => match (l, r) {
                    (Some(l), Some(r)) => (cb.w - l - r - c.margin[1] - c.margin[3]).max(0.0),
                    _ => match self.replaced(ch) {
                        Some((rw, _)) if self.disp(ch) == Disp::InlineBlock => rw,
                        _ => (self.extent(ch, 0) + frame_h).min(cb.w),
                    },
                },
            };
            // a percentage height is a share of the containing block's padding box
            self.ph.push(Some(cb.h));
            let out = self.block(ch, 0.0, 0.0, cb.w, Some(ow), depth + 1);
            self.ph.pop();
            let mut rect = self.rects[ch].unwrap_or_default();
            if matches!(c.height, Length::Auto) {
                if let (Some(t), Some(b)) = (t, b) {
                    rect.h = (cb.h - t - b - c.margin[0] - c.margin[2]).max(frame_v);
                    self.rects[ch] = Some(rect);
                }
            }
            let _ = out;
            let fx = match (l, r) {
                (Some(l), _) => cb.x + l + c.margin[3],
                (None, Some(r)) => cb.x + cb.w - r - c.margin[1] - rect.w,
                _ => sx + c.margin[3],
            };
            let fy = match (t, b) {
                (Some(t), _) => cb.y + t + c.margin[0],
                (None, Some(b)) => cb.y + cb.h - b - c.margin[2] - rect.h,
                _ => sy + c.margin[0],
            };
            self.shift(ch, fx - rect.x, fy - rect.y);
        }
    }

    /// Move a box and every box inside it. An absolute box with no offsets that is still waiting for its containing block stands where
    /// it would have stood in the flow, so it moves with the boxes it was found in.
    fn shift(&mut self, n: usize, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let mut moved = std::collections::HashSet::new();
        let mut stack = vec![n];
        while let Some(m) = stack.pop() {
            moved.insert(m);
            if let Some(r) = self.rects[m].as_mut() {
                r.x += dx;
                r.y += dy;
            }
            stack.extend(self.kids_of(m));
        }
        for frame in self.abs.iter_mut() {
            for (node, sx, sy) in frame.iter_mut() {
                if moved.contains(node) {
                    *sx += dx;
                    *sy += dy;
                }
            }
        }
    }

    fn element_kids(&mut self, n: usize, x: f32, y: f32) -> Vec<usize> {
        let mut out = Vec::new();
        for k in self.kids_of(n) {
            if !matches!(self.tree.nodes[k].kind, NodeKind::Element { .. }) || self.disp(k) == Disp::None {
                continue;
            }
            match self.style(k).map(|c| c.fx.pos) {
                Some(p @ (Pos::Absolute | Pos::Fixed)) => {
                    let frame = if p == Pos::Fixed { 0 } else { self.abs.len() - 1 };
                    self.abs[frame].push((k, x, y));
                }
                _ => out.push(k),
            }
        }
        out
    }

    fn flex_row(&mut self, n: usize, c: &Computed, x: f32, y: f32, w: f32, depth: usize) -> f32 {
        let kids = self.element_kids(n, x, y);
        if kids.is_empty() {
            return 0.0;
        }
        // base widths (border box)
        let mut items: Vec<(usize, f32, f32)> = Vec::new(); // node, base width, grow
        for &k in &kids {
            let ck = self.style(k).cloned().unwrap_or_default();
            let frame = ck.padding[1] + ck.padding[3] + ck.border_w[1] + ck.border_w[3];
            let base = ck.basis.resolve(w, 1.0).or_else(|| ck.width.resolve(w, 1.0)).map(|v| v + frame).unwrap_or_else(|| self.replaced(k).map(|r| r.0).unwrap_or_else(|| self.extent(k, 0) + frame)) + ck.margin[1] + ck.margin[3];
            items.push((k, base, ck.grow));
        }
        // lines
        let mut lines: Vec<Vec<usize>> = vec![Vec::new()];
        let mut used = 0.0f32;
        for (i, it) in items.iter().enumerate() {
            let need = it.1 + if lines.last().is_some_and(|l| !l.is_empty()) { c.gap } else { 0.0 };
            if c.wrap && used + need > w + 0.01 && lines.last().is_some_and(|l| !l.is_empty()) {
                lines.push(Vec::new());
                used = 0.0;
            }
            used += it.1 + if lines.last().is_some_and(|l| !l.is_empty()) { c.gap } else { 0.0 };
            lines.last_mut().unwrap().push(i);
        }
        let mut cursor = 0.0f32;
        for (li, line) in lines.iter().enumerate() {
            let gaps = c.gap * (line.len().saturating_sub(1)) as f32;
            let sum: f32 = line.iter().map(|&i| items[i].1).sum::<f32>() + gaps;
            let free = w - sum;
            let grow_total: f32 = line.iter().map(|&i| items[i].2).sum();
            let mut widths: Vec<f32> = line.iter().map(|&i| items[i].1).collect();
            if free > 0.0 && grow_total > 0.0 {
                for (k, &i) in line.iter().enumerate() {
                    widths[k] += free * items[i].2 / grow_total;
                }
            } else if free < 0.0 && !c.wrap {
                let total: f32 = widths.iter().sum();
                if total > 0.0 {
                    let f = ((w - gaps) / total).max(0.0);
                    for v in widths.iter_mut() {
                        *v *= f;
                    }
                }
            }
            let used: f32 = widths.iter().sum::<f32>() + gaps;
            let spare = (w - used).max(0.0);
            let n_items = line.len() as f32;
            let (mut off, between) = match c.justify {
                Justify::End => (spare, 0.0),
                Justify::Center => (spare / 2.0, 0.0),
                Justify::Between if line.len() > 1 => (0.0, spare / (n_items - 1.0)),
                Justify::Around => (spare / n_items / 2.0, spare / n_items),
                _ => (0.0, 0.0),
            };
            let mut row_h = 0.0f32;
            for (k, &i) in line.iter().enumerate() {
                let node = items[i].0;
                let ck = self.style(node).cloned().unwrap_or_default();
                let bw = (widths[k] - ck.margin[1] - ck.margin[3]).max(0.0);
                let out = self.block(node, x + off, y + cursor, bw + ck.margin[1] + ck.margin[3], Some(bw), depth + 1);
                // `block` put the box after its left margin; the margin box is what the line used
                row_h = row_h.max(out.h + out.mt + out.mb);
                off += widths[k] + c.gap + between;
            }
            cursor += row_h + if li + 1 < lines.len() { c.row_gap.max(c.gap) } else { 0.0 };
        }
        cursor
    }

    fn grid(&mut self, n: usize, c: &Computed, x: f32, y: f32, w: f32, depth: usize) -> f32 {
        let kids = self.element_kids(n, x, y);
        if kids.is_empty() {
            return 0.0;
        }
        let cols = if c.grid_min > 0.0 { (((w + c.gap) / (c.grid_min + c.gap)).floor() as usize).max(1) } else { (c.grid_cols as usize).max(1) };
        let cols = cols.min(kids.len().max(1)).max(1);
        let col_w = ((w - c.gap * (cols as f32 - 1.0)) / cols as f32).max(0.0);
        let mut cursor = 0.0f32;
        for row in kids.chunks(cols) {
            let mut row_h = 0.0f32;
            for (i, &k) in row.iter().enumerate() {
                let out = self.block(k, x + i as f32 * (col_w + c.gap), y + cursor, col_w, Some(col_w), depth + 1);
                row_h = row_h.max(out.h + out.mt + out.mb);
            }
            cursor += row_h + c.row_gap.max(c.gap);
        }
        (cursor - c.row_gap.max(c.gap)).max(0.0)
    }

    /// A table row: cells side by side, the width shared evenly unless a cell says its own.
    /// The column widths of a table: a column takes the widest length any of its cells asks for, the others share what is left.
    fn table_columns(&mut self, table: usize, w: f32) -> Vec<f32> {
        let mut rows: Vec<usize> = Vec::new();
        let mut stack: Vec<usize> = self.kids_of(table);
        stack.reverse();
        while let Some(k) = stack.pop() {
            if !matches!(self.tree.nodes[k].kind, NodeKind::Element { .. }) || self.disp(k) == Disp::None {
                continue;
            }
            match self.disp(k) {
                Disp::Row => rows.push(k),
                _ if matches!(self.tree.tag(k).unwrap_or(""), "thead" | "tbody" | "tfoot") => {
                    let mut kids = self.kids_of(k);
                    kids.reverse();
                    stack.extend(kids);
                }
                _ => {}
            }
        }
        let mut asked: Vec<Option<f32>> = Vec::new();
        for r in rows {
            let cells: Vec<usize> = self.kids_of(r).into_iter().filter(|&k| matches!(self.tree.nodes[k].kind, NodeKind::Element { .. }) && self.disp(k) != Disp::None && !matches!(self.style(k).map(|c| c.fx.pos), Some(Pos::Absolute | Pos::Fixed))).collect();
            for (i, &k) in cells.iter().enumerate() {
                let want = self.style(k).and_then(|c| c.width.resolve(w, 1.0));
                if asked.len() <= i {
                    asked.resize(i + 1, None);
                }
                asked[i] = match (asked[i], want) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
            }
        }
        let fixed: f32 = asked.iter().flatten().sum();
        let free = asked.iter().filter(|a| a.is_none()).count().max(1) as f32;
        let each = ((w - fixed) / free).max(0.0);
        asked.iter().map(|a| a.unwrap_or(each)).collect()
    }

    fn table_row(&mut self, n: usize, x: f32, y: f32, w: f32, depth: usize) -> f32 {
        let kids = self.element_kids(n, x, y);
        if kids.is_empty() {
            return 0.0;
        }
        let fixed: Vec<Option<f32>> = kids.iter().map(|&k| self.style(k).and_then(|c| c.width.resolve(w, 1.0))).collect();
        let fixed_sum: f32 = fixed.iter().flatten().sum();
        let free_cells = fixed.iter().filter(|f| f.is_none()).count().max(1) as f32;
        let each = ((w - fixed_sum) / free_cells).max(0.0);
        // the columns of the table this row is in, when it is in one: every row has the same columns
        let shared = self.cols.last().filter(|c| c.len() >= kids.len()).cloned();
        let mut off = 0.0f32;
        let mut row_h = 0.0f32;
        for (i, &k) in kids.iter().enumerate() {
            let cw = shared.as_ref().map(|c| c[i]).unwrap_or_else(|| fixed[i].unwrap_or(each));
            let out = self.block(k, x + off, y, cw, Some(cw), depth + 1);
            row_h = row_h.max(out.h);
            off += cw;
        }
        row_h
    }
}

/// Does the box have a frame or a layout of its own that keeps its margins from collapsing through it?
fn framed_or_flex(c: &Computed, d: Disp) -> bool {
    c.padding.iter().any(|p| *p > 0.0) || c.border_w.iter().any(|b| *b > 0.0) || !matches!(d, Disp::Block) || c.overflow_hidden || c.fx.float != Float::None || !matches!(c.height, Length::Auto | Length::Px(0.0)) || c.min_height.resolve(0.0, 1.0).is_some_and(|m| m > 0.0)
}

fn is_replaced(tag: &str) -> bool {
    matches!(tag, "img" | "input" | "button" | "select" | "textarea" | "video" | "audio" | "canvas" | "svg" | "iframe" | "object" | "embed")
}
