//! `EditorView`: the IYAKKI text editor as a component an SDK app places in its layout (docs/development/UI-SDK.md, "Embedding the editor",
//! F-SDK-104). The engine is `iyakki-edit` (the buffer with undo, the highlighter, the diff and the proposal reader that the EDIT tile and the
//! chat's edit mode use); the view owns what is on screen: the gutter, the highlighted lines, the cursor and selection, the toolbar and the
//! review of what the assistant proposes. It draws through [`Canvas`], so it works in the shell, in a browser or client and as a WASM tile.
//!
//! What the app does:
//! ```text
//! open a file:   ed.load("notes/todo.md", &text)                     (the app got the text from the files port)
//! event(Action): if ed.owns(a) { ed.action(a) }   event(Key): ed.key(k)   event(Text): ed.text(t)   event(Wheel): ed.wheel(n)
//! view:          ed.draw(c, th, &metrics, rect)
//! saving:        if let Some((path, text)) = ed.take_save() { ... the app writes it through its own port ... ; ed.mark_saved() }
//! the assistant: items = ed.context()                          (the document and the selection, as context for the assistant)
//!                ed.offer_reply(&assistant_reply)               (a reply with SEARCH/REPLACE blocks becomes a proposal to review)
//! ```
//! The editor never writes a file and never calls the assistant: the app does both through the ports it was handed. An assistant's change is
//! never applied by itself: the view shows each changed place as a hunk the person accepts or rejects, then applies the accepted ones as one
//! undoable edit.

use iyakki_edit::buffer::{Buffer, Move, Pos};
use iyakki_edit::diff::{apply_hunks, diff_hunks, summary, Hunk};
use iyakki_edit::proposal::parse_reply;
use iyakki_edit::syntax::{highlight_line, lang_for_path, HlState, Lang, TokenKind};

use crate::canvas::{fit, Canvas, Hit, Role, TextStyle};
use crate::event::UiKey;
use crate::geom::Rect;
use crate::size::{Metrics, TypeRole};
use crate::tokens::{Radius, Space};

use crate::theme::Theme;
use crate::widgets;

/// The widest share of the editor a toolbar button may take.
const TOOL_SHARE: f32 = 0.2;

const A_UNDO: u32 = 0;
const A_REDO: u32 = 1;
const A_SAVE: u32 = 2;
const A_APPLY: u32 = 3;
const A_DISCARD: u32 = 4;
const A_FOCUS: u32 = 5;
const A_HUNK0: u32 = 8;
const A_LINE0: u32 = 40;
/// Hunks listed and text rows drawn at most (a hunk has the action `base + 8 + i`, a text row `base + 40 + i`).
pub const MAX_HUNKS: u32 = 32;
pub const MAX_ROWS: u32 = 120;
/// How many action numbers an editor view uses, from the base the app gave it.
pub const ACTIONS: u32 = A_LINE0 + MAX_ROWS;

/// One assistant proposal under review.
struct Review {
    hunks: Vec<Hunk>,
    accept: Vec<bool>,
    new_lines: Vec<String>,
    note: String,
}

/// What the assistant is given about the document: the same shape as the assistant component's context items, so the app passes them on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorContext {
    pub document: (String, String),
    pub selection: Option<(String, String)>,
}

pub struct EditorView {
    base: u32,
    buf: Buffer,
    pub path: String,
    lang: Lang,
    top: usize,
    pub focused: bool,
    rows: usize,
    save_req: bool,
    review: Option<Review>,
    error: Option<String>,
}

fn kind_colour(th: &Theme, k: TokenKind) -> [u8; 4] {
    match k {
        TokenKind::Keyword => th.accent,
        TokenKind::Type | TokenKind::Function => th.cyan,
        TokenKind::Macro | TokenKind::Attribute => th.magenta,
        TokenKind::String => th.ok,
        TokenKind::Number | TokenKind::Constant => th.amber,
        TokenKind::Comment => th.dim,
        TokenKind::Heading | TokenKind::Key => th.accent,
        TokenKind::Variable => th.text,
    }
}

impl EditorView {
    pub fn new(action_base: u32) -> EditorView {
        EditorView { base: action_base, buf: Buffer::new(), path: String::new(), lang: Lang::Plain, top: 0, focused: false, rows: 10, save_req: false, review: None, error: None }
    }

    pub fn owns(&self, a: u32) -> bool {
        (self.base..self.base + ACTIONS).contains(&a)
    }

    pub fn wants_keyboard(&self) -> bool {
        self.focused
    }

    /// Open `text` as the file `path` (replaces what was there, undo history included). A file that is too big or binary is refused with a sentence.
    pub fn load(&mut self, path: &str, text: &str) -> Result<(), String> {
        match Buffer::from_text(text) {
            Ok(b) => {
                self.buf = b;
                self.path = path.to_string();
                self.lang = lang_for_path(path);
                self.top = 0;
                self.review = None;
                self.error = None;
                self.save_req = false;
                Ok(())
            }
            Err(e) => {
                self.error = Some(e.to_string());
                Err(e.to_string())
            }
        }
    }

    pub fn text(&self) -> String {
        self.buf.text()
    }

    pub fn dirty(&self) -> bool {
        self.buf.is_dirty()
    }

    pub fn cursor(&self) -> Pos {
        self.buf.cursor
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn reviewing(&self) -> bool {
        self.review.is_some()
    }

    /// The number of changed places waiting for the person's yes or no.
    pub fn hunks(&self) -> usize {
        self.review.as_ref().map_or(0, |r| r.hunks.len())
    }

    /// The person pressed Save: the path and the whole text, once. The app writes it and calls [`EditorView::mark_saved`].
    pub fn take_save(&mut self) -> Option<(String, String)> {
        std::mem::take(&mut self.save_req).then(|| (self.path.clone(), self.buf.text()))
    }

    pub fn mark_saved(&mut self) {
        self.buf.mark_saved();
    }

    /// What the assistant should see: the document (at most 40 KB of it, cut at a line) and the selection when there is one. The selection is
    /// the person's own text and the document is data, never instructions: the assistant component marks both so.
    pub fn context(&self) -> EditorContext {
        let name = if self.path.is_empty() { "Document".to_string() } else { self.path.clone() };
        let mut doc = String::new();
        for l in self.buf.lines() {
            if doc.len() + l.len() + 1 > 40 * 1024 {
                doc.push_str("[the rest of the file is not shown]\n");
                break;
            }
            doc.push_str(l);
            doc.push('\n');
        }
        EditorContext { document: (name.clone(), doc), selection: self.buf.selected_text().filter(|t| !t.is_empty()).map(|t| (format!("Selection in {name}"), t.chars().take(4000).collect())) }
    }

    /// The same as [`EditorView::context`] as the assistant component's context items (an instruction item that says how to propose a change,
    /// the document, the selection), ready for `ChatView::set_context`. The instruction item is what makes a reply readable by
    /// [`EditorView::offer_reply`]: the model is told to answer with SEARCH/REPLACE blocks.
    pub fn assistant_context(&self) -> Vec<crate::assistant::ContextItem> {
        use crate::assistant::{ContextItem, ContextKind};
        let c = self.context();
        let mut v = vec![ContextItem::new(ContextKind::Other, "How to change the text", iyakki_edit::proposal::EDIT_INSTRUCTIONS), ContextItem::new(ContextKind::Document, &c.document.0, &c.document.1)];
        if let Some((n, t)) = c.selection {
            v.push(ContextItem::new(ContextKind::Selection, &n, &t));
        }
        v
    }

    /// A reply of the assistant: when it holds SEARCH/REPLACE blocks (or a whole file) that match, they become a proposal to review. Returns
    /// the assistant's own explanation (the reply without the blocks), or `Ok(None)` when it holds no edit, or why it cannot be applied.
    pub fn offer_reply(&mut self, reply: &str) -> Result<Option<String>, String> {
        let current = self.buf.text();
        match parse_reply(reply, &current)? {
            None => Ok(None),
            Some(p) => {
                let old: Vec<String> = current.split('\n').map(str::to_string).collect();
                let new_lines: Vec<String> = p.new_text.split('\n').map(str::to_string).collect();
                let mut hunks = diff_hunks(&old, &new_lines);
                if hunks.is_empty() {
                    return Ok(Some(p.explanation));
                }
                hunks.truncate(MAX_HUNKS as usize);
                let accept = vec![true; hunks.len()];
                let (add, del) = summary(&hunks);
                self.review = Some(Review { hunks, accept, new_lines, note: format!("The assistant proposes {add} added and {del} removed lines") });
                Ok(Some(p.explanation))
            }
        }
    }

    /// Apply the accepted hunks as one undoable edit and end the review.
    pub fn apply_review(&mut self) {
        let Some(r) = self.review.take() else { return };
        let old: Vec<String> = self.buf.text().split('\n').map(str::to_string).collect();
        let _ = &r.new_lines;
        let merged = apply_hunks(&old, &r.hunks, &r.accept).join("\n");
        if let Err(e) = self.buf.set_text_undoable(&merged) {
            self.error = Some(e.to_string());
        }
    }

    pub fn discard_review(&mut self) {
        self.review = None;
    }

    pub fn toggle_hunk(&mut self, i: usize) {
        if let Some(a) = self.review.as_mut().and_then(|r| r.accept.get_mut(i)) {
            *a = !*a;
        }
    }

    fn ensure_visible(&mut self) {
        let line = self.buf.cursor.0;
        let rows = self.rows.max(1);
        if line < self.top {
            self.top = line;
        } else if line >= self.top + rows {
            self.top = line + 1 - rows;
        }
    }

    pub fn action(&mut self, a: u32) -> bool {
        if !self.owns(a) {
            return false;
        }
        match a - self.base {
            A_UNDO => {
                self.buf.undo();
            }
            A_REDO => {
                self.buf.redo();
            }
            A_SAVE => self.save_req = true,
            A_APPLY => self.apply_review(),
            A_DISCARD => self.discard_review(),
            A_FOCUS => self.focused = true,
            h if (A_HUNK0..A_LINE0).contains(&h) => self.toggle_hunk((h - A_HUNK0) as usize),
            l if l >= A_LINE0 => {
                let line = (self.top + (l - A_LINE0) as usize).min(self.buf.line_count().saturating_sub(1));
                let col = self.buf.line(line).chars().count();
                self.buf.set_cursor((line, col), false);
                self.focused = true;
            }
            _ => {}
        }
        self.ensure_visible();
        true
    }

    /// Keys while the editor has the focus: arrows, Enter, Backspace, Delete, Tab (indent), characters; Esc gives the focus back. Returns
    /// whether the key was used.
    pub fn key(&mut self, k: UiKey) -> bool {
        if self.review.is_some() {
            match k {
                UiKey::Enter | UiKey::Char('y') => self.apply_review(),
                UiKey::Esc | UiKey::Char('n') => self.discard_review(),
                _ => return false,
            }
            return true;
        }
        if !self.focused {
            return false;
        }
        let page = self.rows.max(1);
        let r = match k {
            UiKey::Char(c) if !c.is_control() => self.buf.type_char(c).is_ok(),
            UiKey::Enter => self.buf.newline_pair().is_ok(),
            UiKey::Backspace => self.buf.backspace_pair().is_ok(),
            UiKey::Delete => self.buf.delete().is_ok(),
            UiKey::Tab => self.buf.indent().is_ok(),
            UiKey::Left => {
                self.buf.move_to(Move::Left, false);
                true
            }
            UiKey::Right => {
                self.buf.move_to(Move::Right, false);
                true
            }
            UiKey::Up => {
                self.buf.move_to(Move::Up, false);
                true
            }
            UiKey::Down => {
                self.buf.move_to(Move::Down, false);
                true
            }
            UiKey::Esc => {
                self.focused = false;
                true
            }
            _ => return false,
        };
        let _ = page;
        if !r {
            self.error = Some("that edit is refused: the file would be too large".into());
        }
        self.ensure_visible();
        true
    }

    /// Typed or pasted text (control characters other than a line break are dropped).
    pub fn text_in(&mut self, t: &str) -> bool {
        if !self.focused || self.review.is_some() {
            return false;
        }
        let clean: String = t.chars().filter(|c| !c.is_control() || *c == '\n').collect();
        if self.buf.insert(&clean).is_err() {
            self.error = Some("that edit is refused: the file would be too large".into());
        }
        self.ensure_visible();
        true
    }

    pub fn wheel(&mut self, d: i32) {
        let max = self.buf.line_count().saturating_sub(self.rows.max(1));
        self.top = (self.top as i64 - 3 * d.signum() as i64).clamp(0, max as i64) as usize;
    }

    pub fn draw(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, area: Rect) {
        let t = m.min_target();
        widgets::surface(c, th, m, area, th.field, Radius::Lg, true);
        let (xs, sm, md) = (m.gap(Space::Xs), m.gap(Space::Sm), m.gap(Space::Md));
        let inner = area.inset(md);
        // the toolbar: Undo, Redo, Save, and the name
        let bw = m.em(4.0).max(t).min(inner.w * TOOL_SHARE);
        let bar = Rect::new(inner.x, inner.y, inner.w, t);
        widgets::button(c, th, m, Rect::new(bar.x, bar.y, bw, t), "Undo", self.base + A_UNDO, false);
        widgets::button(c, th, m, Rect::new(bar.x + bw + xs, bar.y, bw, t), "Redo", self.base + A_REDO, false);
        widgets::button(c, th, m, Rect::new(bar.x + 2.0 * (bw + xs), bar.y, bw, t), "Save", self.base + A_SAVE, self.buf.is_dirty());
        let size = m.size(TypeRole::Small);
        let name = format!("{}{}", if self.path.is_empty() { "(no file)" } else { &self.path }, if self.buf.is_dirty() { " *" } else { "" });
        let nx = bar.x + (bw + xs) + (bw + xs) + (bw + xs) + xs;
        if bar.right() > nx + m.gap(Space::Xl) + xs {
            let tx = fit(c, size, &name, TextStyle::Bold, bar.right() - nx);
            c.text(nx, bar.y + ((t - size) / 2.0).max(0.0), size, th.muted, &tx, TextStyle::Bold);
        }
        let mut top = bar.bottom() + sm;
        if let Some(e) = self.error.clone() {
            let s = m.size(TypeRole::Caption);
            let tx = fit(c, s, &e, TextStyle::Regular, inner.w);
            c.text(inner.x, top, s, th.err, &tx, TextStyle::Regular);
            top += m.leading(s) + xs;
        }
        // the review of the assistant's proposal, under the text
        let mut bottom = inner.bottom();
        if let Some(r) = &self.review {
            let rows = r.hunks.len().min(4);
            let hh = t.max(m.leading(m.size(TypeRole::Body)) * 1.25);
            let card_h = (t + md) + rows as f32 * (hh + m.gap(Space::Xxs)) + m.leading(m.size(TypeRole::Small)) + m.gap(Space::Lg);
            if bottom - card_h > top + t * 2.0 {
                let card = Rect::new(inner.x, bottom - card_h, inner.w, card_h);
                let rad = widgets::surface(c, th, m, card, th.card_sel, Radius::Md, true);
                c.outline(card, rad, m.tokens.focus_width(), th.amber);
                let sf = m.size(TypeRole::Small);
                let note = fit(c, sf, &r.note, TextStyle::Bold, card.w - 2.0 * md);
                c.text(card.x + md, card.y + sm, sf, th.text, &note, TextStyle::Bold);
                let mut y = card.y + sm + m.leading(sf);
                for (i, h) in r.hunks.iter().enumerate().take(rows) {
                    let rr = Rect::new(card.x + md, y, card.w - 2.0 * md, hh);
                    let on = r.accept[i];
                    widgets::surface(c, th, m, rr, if on { th.card } else { th.well }, Radius::Sm, false);
                    let label = format!("{} line {}: -{} +{}", if on { "Accept" } else { "Skip" }, h.old_start + 1, h.old.len(), h.new.len());
                    let tx = fit(c, sf, &label, TextStyle::Regular, rr.w - 2.0 * md);
                    c.text(rr.x + md, rr.y + ((hh - sf) / 2.0).max(0.0), sf, if on { th.text } else { th.dim }, &tx, TextStyle::Regular);
                    c.hit(rr, Hit { action: self.base + A_HUNK0 + i as u32, name: label, role: Role::Button, ..Default::default() });
                    y += hh + m.gap(Space::Xxs);
                }
                let by = card.bottom() - t - sm;
                let w2 = ((card.w - md - 2.0 * md) / 2.0).max(0.0);
                widgets::button(c, th, m, Rect::new(card.x + md, by, w2, t), "Apply", self.base + A_APPLY, true);
                widgets::button(c, th, m, Rect::new(card.x + 2.0 * md + w2, by, w2, t), "Discard", self.base + A_DISCARD, false);
                bottom = card.y - sm;
            }
        }
        // the text: a gutter with line numbers, then the highlighted lines
        let fs = m.size(TypeRole::Small);
        let lh = (fs * 1.45).max(t.min(fs * 1.9));
        let text_area = Rect::new(inner.x, top, inner.w, (bottom - top).max(0.0));
        let rows = ((text_area.h / lh).floor() as usize).clamp(1, MAX_ROWS as usize);
        self.rows = rows;
        self.top = self.top.min(self.buf.line_count().saturating_sub(rows));
        let digits = self.buf.line_count().to_string().len().max(2);
        let cw = c.measure(fs, "M", TextStyle::Mono).max(1.0);
        let gutter = (digits as f32 + 1.5) * cw;
        let (cur, sel) = (self.buf.cursor, self.buf.selection());
        // the highlighter state at the top line: run the lines above it (cheap: it only carries a small state)
        let mut st = HlState::Normal;
        for l in 0..self.top.min(self.buf.line_count()) {
            st = highlight_line(self.lang, self.buf.line(l), st).1;
        }
        let first = self.top;
        c.clipped(text_area, &mut |c| {
            let mut st = st;
            for r in 0..rows {
                let ln = first + r;
                if ln >= self.buf.line_count() {
                    break;
                }
                let y = text_area.y + r as f32 * lh;
                let row = Rect::new(text_area.x, y, text_area.w, lh);
                if ln == cur.0 {
                    c.rect(row, th.card);
                }
                c.text(text_area.x, y + (lh - fs) / 2.0, fs, th.dim, &format!("{:>width$}", ln + 1, width = digits), TextStyle::Mono);
                let line = self.buf.line(ln);
                let (spans, next) = highlight_line(self.lang, line, st);
                st = next;
                let chars: Vec<char> = line.chars().collect();
                let avail = (((text_area.w - gutter) / cw).floor() as usize).max(1);
                // the part of the line that fits (a long line is cut at the right edge; the caret keeps its column in view)
                let start = if ln == cur.0 && cur.1 > avail.saturating_sub(2) { cur.1 + 2 - avail } else { 0 };
                let shown: Vec<char> = chars.iter().copied().skip(start).take(avail).collect();
                // the selection behind the text
                if let Some((a, b)) = sel {
                    if ln >= a.0 && ln <= b.0 {
                        let (from, to) = (if ln == a.0 { a.1 } else { 0 }, if ln == b.0 { b.1 } else { chars.len() });
                        let (x0, x1) = (from.max(start), to.min(start + shown.len()));
                        if x1 > x0 {
                            c.rect(Rect::new(text_area.x + gutter + (x0 - start) as f32 * cw, y, (x1 - x0) as f32 * cw, lh), th.card_sel);
                        }
                    }
                }
                // runs of one colour
                let colour_at = |i: usize| spans.iter().find(|s| i >= s.start && i < s.end).map_or(th.text, |s| kind_colour(th, s.kind));
                let mut i = 0;
                while i < shown.len() {
                    let col = colour_at(start + i);
                    let mut j = i + 1;
                    while j < shown.len() && colour_at(start + j) == col {
                        j += 1;
                    }
                    let run: String = shown[i..j].iter().collect();
                    c.text(text_area.x + gutter + i as f32 * cw, y + (lh - fs) / 2.0, fs, col, &run, TextStyle::Mono);
                    i = j;
                }
                if ln == cur.0 && self.focused {
                    let cx = text_area.x + gutter + cur.1.saturating_sub(start) as f32 * cw;
                    c.rect(Rect::new(cx, y + 1.0, 2.0, lh - 2.0), th.accent);
                }
                c.hit(row, Hit { action: self.base + A_LINE0 + r as u32, name: format!("line {}", ln + 1), role: Role::ListItem, ..Default::default() });
            }
        });
    }
}
