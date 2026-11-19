//! How a page looks for a person who needs more contrast or bigger words: a forced-colours palette that replaces every colour the page
//! chose (text, links, visited links, backgrounds, borders) and a minimum font size. Both work on the finished [`Doc`], like night
//! colours, and are pure so that tests can compute the contrast of the result (WCAG 2 ratios).

use crate::css::Rgba;
use crate::doc::{Block, Column, Doc, Span};
use crate::style::contrast;

/// The colours of forced-colours mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub bg: Rgba,
    pub fg: Rgba,
    pub link: Rgba,
    pub visited: Rgba,
    /// The ring round the key-focused element (against the page background).
    pub focus: Rgba,
    pub border: Rgba,
}

impl Palette {
    /// Black, white and yellow: every pair above 7 to 1.
    pub const BLACK_ON_WHITE_LIKE: Palette = Palette { bg: [0, 0, 0, 255], fg: [255, 255, 255, 255], link: [255, 221, 0, 255], visited: [0, 230, 255, 255], focus: [255, 221, 0, 255], border: [255, 255, 255, 255] };

    /// The first pair (name, ratio, needed) that does not reach WCAG AA (4.5 for text, 3 for the ring and borders); `Ok` when all do.
    pub fn check(&self) -> Result<(), String> {
        let pairs = [("text", self.fg, 4.5), ("link", self.link, 4.5), ("visited link", self.visited, 4.5), ("focus ring", self.focus, 3.0), ("border", self.border, 3.0)];
        for (name, c, need) in pairs {
            let r = contrast(c, self.bg);
            if r < need {
                return Err(format!("{name} has contrast {r:.2}, needs {need}"));
            }
        }
        // a visited link must not be confused with an ordinary one
        if self.link == self.visited {
            return Err("visited and unvisited links have the same colour".into());
        }
        Ok(())
    }

    /// The smallest ratio of the palette's text colours against its background (for tests and the status line).
    pub fn lowest_text_ratio(&self) -> f32 {
        [self.fg, self.link, self.visited].iter().map(|c| contrast(*c, self.bg)).fold(f32::MAX, f32::min)
    }
}

/// What to change.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Adapt {
    pub palette: Option<Palette>,
    /// A floor for text, in pixels at normal zoom (0: none). The page's body text is 16.
    pub min_font_px: u16,
}

impl Adapt {
    pub fn is_off(&self) -> bool {
        self.palette.is_none() && self.min_font_px == 0
    }
}

fn recolour(spans: &mut [Span], doc_links: &[String], pal: &Palette, visited: &dyn Fn(&str) -> bool) {
    for s in spans {
        match s.link {
            Some(l) => {
                let seen = doc_links.get(l).is_some_and(|h| visited(h));
                s.style.color = Some(if seen { pal.visited } else { pal.link });
                s.style.underline = Some(true);
            }
            None => s.style.color = Some(pal.fg),
        }
        s.style.bg = None;
    }
}

fn floor(spans: &mut [Span], min_pct: u16, level: u8) {
    for s in spans {
        if level == 0 {
            // text with no size of its own is the body size
            if s.style.size_pct == 0 && min_pct > 100 {
                s.style.size_pct = min_pct;
            }
        }
        if s.style.size_pct > 0 && s.style.size_pct < min_pct {
            s.style.size_pct = min_pct;
        }
    }
}

fn walk(blocks: &mut [Block], links: &[String], a: &Adapt, min_pct: u16, visited: &dyn Fn(&str) -> bool) {
    for b in blocks {
        match b {
            Block::Text(t) => {
                if let Some(p) = &a.palette {
                    recolour(&mut t.spans, links, p, visited);
                }
                if min_pct > 0 {
                    floor(&mut t.spans, min_pct, t.level);
                }
            }
            Block::Box(x) => {
                if let Some(p) = &a.palette {
                    x.style.bg = None;
                    x.style.border_c = x.style.border_c.map(|_| p.border);
                    x.style.fx.shadow = None;
                    x.style.fx.opacity = 1.0;
                    x.style.fx.gradient = None;
                }
                walk(&mut x.children, links, a, min_pct, visited);
            }
            Block::Row(r) => {
                if let Some(p) = &a.palette {
                    r.style.bg = None;
                    r.style.border_c = r.style.border_c.map(|_| p.border);
                }
                for c in &mut r.cols {
                    let Column { style, blocks: inner, .. } = c;
                    if let Some(p) = &a.palette {
                        style.bg = None;
                        style.border_c = style.border_c.map(|_| p.border);
                    }
                    walk(inner, links, a, min_pct, visited);
                }
            }
            Block::Table(t) => {
                if a.palette.is_some() {
                    t.bg = None;
                    t.lines = true;
                }
                for row in &mut t.rows {
                    for cell in row {
                        if a.palette.is_some() {
                            cell.bg = None;
                        }
                        walk(&mut cell.blocks, links, a, min_pct, visited);
                    }
                }
            }
            Block::Pre(_) | Block::Rule => {}
        }
    }
}

/// The page with forced colours and a minimum font size, or `None` when `a` asks for nothing. `visited` says whether an address (as
/// written on the page) was visited, for the visited link colour.
pub fn adapt(doc: &Doc, a: &Adapt, visited: &dyn Fn(&str) -> bool) -> Option<Doc> {
    if a.is_off() {
        return None;
    }
    let mut d = doc.clone();
    let min_pct = if a.min_font_px == 0 { 0 } else { ((a.min_font_px as u32 * 100 + 8) / 16).min(400) as u16 };
    if let Some(p) = &a.palette {
        d.canvas = Some(p.bg);
    }
    let links = d.links.clone();
    walk(&mut d.blocks, &links, a, min_pct, visited);
    for b in &mut d.inlines {
        if let Some(p) = &a.palette {
            b.style.bg = None;
            b.style.border_c = b.style.border_c.map(|_| p.border);
        }
        walk(&mut b.children, &links, a, min_pct, visited);
    }
    Some(d)
}

/// Every text colour of the adapted page against its background (the canvas, or the nearest box behind it), for tests: the lowest ratio.
pub fn lowest_text_ratio(doc: &Doc) -> f32 {
    fn spans_min(blocks: &[Block], bg: Rgba, out: &mut f32) {
        for b in blocks {
            match b {
                Block::Text(t) => {
                    for s in &t.spans {
                        if s.text.trim().is_empty() {
                            continue;
                        }
                        let c = s.style.color.unwrap_or([255, 255, 255, 255]);
                        let back = s.style.bg.unwrap_or(bg);
                        *out = out.min(contrast(c, back));
                    }
                }
                Block::Box(x) => spans_min(&x.children, x.style.bg.unwrap_or(bg), out),
                Block::Row(r) => r.cols.iter().for_each(|c| spans_min(&c.blocks, c.style.bg.or(r.style.bg).unwrap_or(bg), out)),
                Block::Table(t) => t.rows.iter().for_each(|r| r.iter().for_each(|c| spans_min(&c.blocks, c.bg.or(t.bg).unwrap_or(bg), out))),
                _ => {}
            }
        }
    }
    let mut lowest = f32::MAX;
    spans_min(&doc.blocks, doc.canvas.unwrap_or([0, 0, 0, 255]), &mut lowest);
    lowest
}
