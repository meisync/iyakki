//! Text selection over read-only text that was laid out line by line (the chat transcript, scrolling logs, reports).
//!
//! A view describes what it drew as a [`SelDoc`]: lines with the position of every character. [`Selection`] turns presses,
//! drags, double and triple clicks into a range over that document and gives back the selected text (plain text, one
//! line break per line shown, a blank line between separate blocks) and the pixel span to highlight on each line.
//! Nothing here draws or talks to the clipboard; the views do.

use crate::gfx::{Face, Gfx};

/// One line of text as drawn. `xs[i]` is the left edge of character `i` and `xs[n]` the right edge of the last one, all
/// in the document's own coordinates (the view decides what those are; they only have to match what it passes to `pos_at`).
#[derive(Clone, Debug)]
pub struct SelLine {
    pub y: f32,
    pub h: f32,
    pub text: String,
    pub xs: Vec<f32>,
    /// Lines of one block are separated by a single line break when copied, separate blocks by a blank line.
    pub block: u32,
    /// What a triple click selects: all lines with the same paragraph number.
    pub para: u32,
}

impl SelLine {
    /// How many characters the line has.
    pub fn len(&self) -> usize {
        if self.xs.is_empty() {
            self.text.chars().count()
        } else {
            self.xs.len() - 1
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A character position: the line (index into the document) and the character before which the caret sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub ch: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SelDoc {
    pub lines: Vec<SelLine>,
}

/// The left edges of the characters of `text` drawn from `x0` (and the right edge of the last one). Text that needs shaping
/// is spread evenly over its measured width.
pub fn char_xs(g: &mut Gfx, x0: f32, size: f32, face: Face, text: &str) -> Vec<f32> {
    let n = text.chars().count();
    let mut xs = Vec::with_capacity(n + 1);
    xs.push(x0);
    if crate::shaping::needs_shaping(text) {
        let w = g.text_width(size, text, face);
        for i in 1..=n {
            xs.push(x0 + w * i as f32 / n.max(1) as f32);
        }
        return xs;
    }
    let mut x = x0;
    let mut buf = [0u8; 4];
    for c in text.chars() {
        x += g.text_width(size, c.encode_utf8(&mut buf), face);
        xs.push(x);
    }
    xs
}

#[derive(PartialEq, Clone, Copy)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

impl SelDoc {
    pub fn new() -> SelDoc {
        SelDoc::default()
    }

    pub fn push(&mut self, l: SelLine) {
        self.lines.push(l);
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The first and the last position of the document.
    pub fn bounds(&self) -> Option<(Pos, Pos)> {
        let last = self.lines.len().checked_sub(1)?;
        Some((Pos { line: 0, ch: 0 }, Pos { line: last, ch: self.lines[last].len() }))
    }

    /// The position nearest to a point: the line whose rows contain `y` (or the nearest one), the character boundary nearest to `x`.
    pub fn pos_at(&self, x: f32, y: f32) -> Option<Pos> {
        if self.lines.is_empty() {
            return None;
        }
        // lines are in reading order; find the first whose bottom is below y
        let i = self.lines.partition_point(|l| l.y + l.h <= y);
        let line = if i >= self.lines.len() {
            self.lines.len() - 1
        } else if i > 0 && y < self.lines[i].y && (y - (self.lines[i - 1].y + self.lines[i - 1].h)) < (self.lines[i].y - y) {
            i - 1
        } else {
            i
        };
        let l = &self.lines[line];
        let n = l.len();
        if l.xs.is_empty() {
            // a line scrolled out of view has no geometry: the nearest end
            return Some(Pos { line, ch: if x <= 0.0 { 0 } else { n } });
        }
        let mut ch = n;
        for k in 0..n {
            if x < (l.xs[k] + l.xs[k + 1]) / 2.0 {
                ch = k;
                break;
            }
        }
        Some(Pos { line, ch })
    }

    fn chars_of(&self, line: usize) -> Vec<char> {
        self.lines[line].text.chars().collect()
    }

    /// The word (or run of spaces, or single symbol) around `p`.
    pub fn word_at(&self, p: Pos) -> (Pos, Pos) {
        let c = self.chars_of(p.line);
        if c.is_empty() {
            return (p, p);
        }
        let i = p.ch.min(c.len() - 1);
        let k = class(c[i]);
        let (mut a, mut b) = (i, i + 1);
        if k != Class::Other {
            while a > 0 && class(c[a - 1]) == k {
                a -= 1;
            }
            while b < c.len() && class(c[b]) == k {
                b += 1;
            }
        }
        (Pos { line: p.line, ch: a }, Pos { line: p.line, ch: b })
    }

    /// The whole paragraph around `p`.
    pub fn para_at(&self, p: Pos) -> (Pos, Pos) {
        let id = self.lines[p.line].para;
        let mut a = p.line;
        while a > 0 && self.lines[a - 1].para == id {
            a -= 1;
        }
        let mut b = p.line;
        while b + 1 < self.lines.len() && self.lines[b + 1].para == id {
            b += 1;
        }
        (Pos { line: a, ch: 0 }, Pos { line: b, ch: self.lines[b].len() })
    }

    /// The text between two positions (in either order).
    pub fn text(&self, a: Pos, b: Pos) -> String {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let mut out = String::new();
        for i in a.line..=b.line.min(self.lines.len().saturating_sub(1)) {
            let l = &self.lines[i];
            let from = if i == a.line { a.ch } else { 0 };
            let to = if i == b.line { b.ch } else { l.len() };
            if i > a.line {
                out.push('\n');
                if l.block != self.lines[i - 1].block {
                    out.push('\n');
                }
            }
            out.extend(l.text.chars().skip(from).take(to.saturating_sub(from)));
        }
        out
    }

    /// The span (left, right) to highlight on `line` for the range `a..b` (ordered).
    pub fn span(&self, line: usize, a: Pos, b: Pos) -> Option<(f32, f32)> {
        if line < a.line || line > b.line {
            return None;
        }
        let l = self.lines.get(line)?;
        if l.xs.is_empty() {
            return None;
        }
        let from = if line == a.line { a.ch } else { 0 };
        let to = if line == b.line { b.ch } else { l.len() };
        (to > from).then(|| (l.xs[from.min(l.len())], l.xs[to.min(l.len())]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Unit {
    #[default]
    Char,
    Word,
    Para,
}

/// A selection over a [`SelDoc`]: where the press began and where the pointer is now.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    anchor: Option<Pos>,
    head: Pos,
    unit: Unit,
    origin: Option<(Pos, Pos)>,
}

impl Selection {
    pub fn clear(&mut self) {
        *self = Selection::default();
    }

    /// The selected range, ordered; `None` when nothing is selected.
    pub fn range(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        (a != self.head).then(|| if a <= self.head { (a, self.head) } else { (self.head, a) })
    }

    pub fn is_empty(&self) -> bool {
        self.range().is_none()
    }

    /// The selected text.
    pub fn text(&self, doc: &SelDoc) -> Option<String> {
        let (a, b) = self.range()?;
        if a.line >= doc.lines.len() {
            return None;
        }
        Some(doc.text(a, Pos { line: b.line.min(doc.lines.len() - 1), ch: b.ch }))
    }

    /// A press at (`x`, `y`): `clicks` is 1, 2 (a word) or 3 (a paragraph); with `shift` the selection grows from where it began.
    pub fn press(&mut self, doc: &SelDoc, x: f32, y: f32, clicks: u8, shift: bool) {
        let Some(p) = doc.pos_at(x, y) else {
            self.clear();
            return;
        };
        if shift && self.anchor.is_some() {
            self.head = p;
            self.unit = Unit::Char;
            self.origin = None;
            return;
        }
        match clicks {
            2 => {
                let (a, b) = doc.word_at(p);
                self.anchor = Some(a);
                self.head = b;
                self.unit = Unit::Word;
                self.origin = Some((a, b));
            }
            3 => {
                let (a, b) = doc.para_at(p);
                self.anchor = Some(a);
                self.head = b;
                self.unit = Unit::Para;
                self.origin = Some((a, b));
            }
            _ => {
                self.anchor = Some(p);
                self.head = p;
                self.unit = Unit::Char;
                self.origin = None;
            }
        }
    }

    /// The pointer moved to (`x`, `y`) with the button held.
    pub fn drag(&mut self, doc: &SelDoc, x: f32, y: f32) {
        let (Some(_), Some(p)) = (self.anchor, doc.pos_at(x, y)) else { return };
        match (self.unit, self.origin) {
            (Unit::Char, _) | (_, None) => self.head = p,
            (unit, Some((o0, o1))) => {
                let (a, b) = if unit == Unit::Word { doc.word_at(p) } else { doc.para_at(p) };
                if p < o0 {
                    self.anchor = Some(o1);
                    self.head = a;
                } else {
                    self.anchor = Some(o0);
                    self.head = b;
                }
            }
        }
    }

    pub fn select_all(&mut self, doc: &SelDoc) {
        match doc.bounds() {
            Some((a, b)) => {
                self.anchor = Some(a);
                self.head = b;
                self.unit = Unit::Char;
                self.origin = None;
            }
            None => self.clear(),
        }
    }

    /// Where to paint the highlight on `line`.
    pub fn span(&self, doc: &SelDoc, line: usize) -> Option<(f32, f32)> {
        let (a, b) = self.range()?;
        doc.span(line, a, b)
    }

    /// Keeps the range inside a document that changed.
    pub fn clamp(&mut self, doc: &SelDoc) {
        let fix = |p: Pos| match doc.lines.len().checked_sub(1) {
            None => Pos::default(),
            Some(last) if p.line > last => Pos { line: last, ch: doc.lines[last].len() },
            Some(_) => Pos { line: p.line, ch: p.ch.min(doc.lines[p.line].len()) },
        };
        if let Some(a) = self.anchor {
            self.anchor = Some(fix(a));
        }
        self.head = fix(self.head);
        if doc.is_empty() {
            self.clear();
        }
    }
}

// ------------------------------------------------------------------------------------------------------- pane

use crate::ui::{Event, Key, R};

/// Selection for a plain text view that draws its lines itself, every frame (a log, a report, a details pane): the view calls
/// [`Pane::begin`], hands every line it draws to [`Pane::line`] (also those scrolled out of view, so line numbers stay put),
/// then [`Pane::end`], which registers the area for presses and paints the highlight. Events come back through [`Pane::handle`].
/// Positions are screen pixels of the last frame.
#[derive(Debug, Default)]
pub struct Pane {
    pub id: usize,
    area: Option<R>,
    doc: SelDoc,
    sel: Selection,
    auto: f32,
    ptr: Option<(f32, f32)>,
}

impl Pane {
    pub fn new(id: usize) -> Pane {
        Pane { id, ..Pane::default() }
    }

    /// Start of a frame: `area` is where the lines are drawn.
    pub fn begin(&mut self, area: R) {
        self.area = Some(area);
        self.doc.lines.clear();
    }

    /// Records a line drawn at (`x`, `y`) in `face` at `size` (`h` tall). Lines far outside the area keep no per-character
    /// geometry. `block` separates blocks when copying (a blank line between), `para` is what a triple click selects.
    #[allow(clippy::too_many_arguments)]
    pub fn line(&mut self, g: &mut Gfx, x: f32, y: f32, h: f32, size: f32, face: Face, text: &str, block: u32, para: u32) {
        let near = self.area.is_some_and(|a| y + h >= a.y - h && y <= a.y + a.h + h);
        let xs = if near { char_xs(g, x, size, face, text) } else { Vec::new() };
        self.doc.push(SelLine { y, h, text: text.to_string(), xs, block, para });
    }

    /// End of a frame: registers the area and paints the selection over the text.
    pub fn end(&mut self, g: &mut Gfx, accent: [u8; 4]) {
        let Some(area) = self.area else { return };
        g.hit(area, Event::Area(self.id), "text area");
        self.sel.clamp(&self.doc);
        for i in 0..self.doc.lines.len() {
            if let Some((a, b)) = self.sel.span(&self.doc, i) {
                let l = &self.doc.lines[i];
                if l.y + l.h >= area.y && l.y <= area.y + area.h {
                    g.rrect(a, l.y, (b - a).max(3.0), l.h, 2.0, [accent[0], accent[1], accent[2], 96]);
                }
            }
        }
    }

    /// The line (counting the lines handed to [`Pane::line`] this frame) and character under a point given relative to the
    /// area (what `Event::AreaAt` carries): used to start reading from a click.
    pub fn locate(&self, xr: f32, yr: f32) -> Option<(usize, usize)> {
        let a = self.area?;
        self.doc.pos_at(a.x + xr, a.y + yr).map(|p| (p.line, p.ch))
    }

    pub fn has_selection(&self) -> bool {
        !self.sel.is_empty()
    }

    pub fn has_text(&self) -> bool {
        !self.doc.is_empty()
    }

    pub fn selected_text(&self) -> Option<String> {
        self.sel.text(&self.doc)
    }

    pub fn clear(&mut self) {
        self.sel.clear();
    }

    pub fn select_all(&mut self) {
        self.sel.select_all(&self.doc);
    }

    /// The whole text, as it would be copied after Select all.
    pub fn all_text(&self) -> Option<String> {
        let (a, b) = self.doc.bounds()?;
        Some(self.doc.text(a, b))
    }

    /// Is the screen point inside the area?
    pub fn over(&self, x: i32, y: i32) -> bool {
        self.area.is_some_and(|a| a.contains(x, y))
    }

    /// How far to scroll on this heartbeat while a drag is held above (positive: towards earlier lines) or below (negative) the area.
    pub fn auto_scroll(&self) -> f32 {
        self.auto
    }

    /// Re-extends the selection to the held pointer after the view scrolled by itself.
    pub fn refresh_drag(&mut self) {
        if let (Some((x, y)), Some(a)) = (self.ptr, self.area) {
            self.sel.drag(&self.doc, x, y.clamp(a.y, a.y + a.h));
        }
    }

    /// Presses and drags in the area, the release, and Ctrl+C with a selection. True when the event was used.
    pub fn handle(&mut self, ev: &Event) -> bool {
        match ev {
            Event::AreaAt(id, xr, yr) if *id == self.id => {
                let Some(a) = self.area else { return true };
                let (x, y) = (a.x + xr, a.y + yr);
                if crate::ui::dragging() {
                    self.ptr = Some((x, y));
                    self.auto = if y < a.y {
                        (6.0 + (a.y - y) * 0.4).min(60.0)
                    } else if y > a.y + a.h {
                        -(6.0 + (y - a.y - a.h) * 0.4).min(60.0)
                    } else {
                        0.0
                    };
                    self.sel.drag(&self.doc, x, y.clamp(a.y, a.y + a.h));
                } else {
                    self.sel.press(&self.doc, x, y, crate::ui::click_count(), crate::ui::shift_held());
                    self.ptr = Some((x, y));
                    self.auto = 0.0;
                }
                true
            }
            Event::MouseUp(..) => {
                self.auto = 0.0;
                self.ptr = None;
                false
            }
            Event::Key(Key::Ctrl('c')) if self.has_selection() => {
                if let Some(t) = self.selected_text() {
                    crate::ui::clipboard::copy(&t);
                }
                true
            }
            _ => false,
        }
    }

    /// Copy and Select all for the context menu, when the pointer is over the text.
    pub fn context_items(&self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        use crate::ctxmenu::{Item, COPY, SELECT_ALL};
        if !self.over(x, y) {
            return Vec::new();
        }
        vec![Item::new(COPY, "Copy", "Ctrl+C").enabled(self.has_selection()), Item::new(SELECT_ALL, "Select all", "Ctrl+A").enabled(self.has_text())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines of monospace text, 10 px per character, 20 px tall; `(text, block, para)`.
    fn doc(lines: &[(&str, u32, u32)]) -> SelDoc {
        let mut d = SelDoc::new();
        for (i, (t, block, para)) in lines.iter().enumerate() {
            let n = t.chars().count();
            d.push(SelLine { y: i as f32 * 20.0, h: 20.0, text: t.to_string(), xs: (0..=n).map(|k| k as f32 * 10.0).collect(), block: *block, para: *para });
        }
        d
    }

    #[test]
    fn a_drag_across_two_blocks_gives_the_text_between_with_a_blank_line() {
        let d = doc(&[("hello world", 0, 0), ("second line", 0, 0), ("other block", 1, 1)]);
        let mut s = Selection::default();
        s.press(&d, 60.0, 5.0, 1, false); // before 'w'
        assert!(s.is_empty(), "a plain press selects nothing yet");
        s.drag(&d, 30.0, 45.0); // 'oth' of the third line
        assert_eq!(s.text(&d).unwrap(), "world\nsecond line\n\noth");
        // dragging back up flips the range
        s.drag(&d, 20.0, 5.0);
        assert_eq!(s.text(&d).unwrap(), "llo ");
    }

    #[test]
    fn double_click_selects_a_word_triple_a_paragraph_and_shift_extends() {
        let d = doc(&[("alpha beta_1 gamma", 0, 0), ("wrapped on", 0, 0), ("next", 0, 1)]);
        let mut s = Selection::default();
        s.press(&d, 75.0, 5.0, 2, false);
        assert_eq!(s.text(&d).unwrap(), "beta_1");
        // dragging after a double click grows by whole words
        s.drag(&d, 165.0, 5.0);
        assert_eq!(s.text(&d).unwrap(), "beta_1 gamma");
        s.press(&d, 5.0, 25.0, 3, false);
        assert_eq!(s.text(&d).unwrap(), "alpha beta_1 gamma\nwrapped on", "the wrapped lines are one paragraph");
        s.press(&d, 0.0, 45.0, 1, false);
        s.press(&d, 20.0, 45.0, 1, true);
        assert_eq!(s.text(&d).unwrap(), "ne");
        s.press(&d, 100.0, 5.0, 1, true);
        assert_eq!(s.text(&d).unwrap(), "_1 gamma\nwrapped on\n");
    }
}
