//! A small multi-line text box for queries (PromQL, SQL, Elasticsearch JSON): the text engine of the editor
//! (`iyakki_edit::Buffer`: cursor, selection, undo, brackets) and a plain monospace drawing of it. No files, no dialogs.

use iyakki_edit::{Buffer, Move};

use crate::fx::{self, with_alpha};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, Nav, DIM, MUTED, R, TEXT};

pub const MAX_CHARS: usize = 20_000;

pub struct QueryBox {
    pub buf: Buffer,
    scroll: usize,
    hscroll: usize,
    /// The id of the press area (the owner decodes `AreaAt(id, x, y)` and calls [`QueryBox::click`]).
    pub area: usize,
    geo: (f32, f32, usize, usize),
}

impl QueryBox {
    pub fn new(area: usize) -> QueryBox {
        QueryBox { buf: Buffer::new(), scroll: 0, hscroll: 0, area, geo: (8.0, 16.0, 40, 8) }
    }

    pub fn text(&self) -> String {
        self.buf.text().trim_end_matches('\n').to_string()
    }

    pub fn set_text(&mut self, t: &str) {
        let t: String = t.chars().take(MAX_CHARS).collect();
        self.buf = Buffer::from_text(&t).unwrap_or_default();
        self.scroll = 0;
        self.hscroll = 0;
    }

    /// Insert at the caret (a name picked from the schema list).
    pub fn insert(&mut self, t: &str) {
        let _ = self.buf.insert(t);
    }

    fn nav(&mut self, nav: Nav, shift: bool, ctrl: bool) {
        let rows = self.geo.3.max(1);
        let m = match (nav, ctrl) {
            (Nav::Left, true) => Move::WordLeft,
            (Nav::Right, true) => Move::WordRight,
            (Nav::Home, true) => Move::DocStart,
            (Nav::End, true) => Move::DocEnd,
            (Nav::Left, _) => Move::Left,
            (Nav::Right, _) => Move::Right,
            (Nav::Up, _) => Move::Up,
            (Nav::Down, _) => Move::Down,
            (Nav::Home, _) => Move::Home,
            (Nav::End, _) => Move::End,
            (Nav::PageUp, _) => Move::PageUp(rows),
            (Nav::PageDown, _) => Move::PageDown(rows),
        };
        self.buf.move_to(m, shift);
    }

    /// Keys and pastes. Returns true when the box used the event. Enter, Esc, Ctrl+Enter and Ctrl+S are left to the owner.
    pub fn handle(&mut self, ev: &Event) -> bool {
        match ev {
            Event::Key(k) => match k {
                Key::Char(c) if !c.is_control() => {
                    if self.buf.byte_len() < MAX_CHARS {
                        let _ = self.buf.type_char(*c);
                    }
                }
                Key::Enter => {
                    let _ = self.buf.newline();
                }
                Key::Backspace => {
                    let _ = self.buf.backspace_pair();
                }
                Key::Delete => {
                    let _ = self.buf.delete();
                }
                Key::Tab => {
                    let _ = self.buf.insert("  ");
                }
                Key::Left => self.nav(Nav::Left, false, false),
                Key::Right => self.nav(Nav::Right, false, false),
                Key::Up => self.nav(Nav::Up, false, false),
                Key::Down => self.nav(Nav::Down, false, false),
                Key::Home => self.nav(Nav::Home, false, false),
                Key::End => self.nav(Nav::End, false, false),
                Key::PageUp => self.nav(Nav::PageUp, false, false),
                Key::PageDown => self.nav(Nav::PageDown, false, false),
                Key::Nav { nav, shift, ctrl } => self.nav(*nav, *shift, *ctrl),
                Key::Ctrl('a') => self.buf.select_all(),
                Key::Ctrl('z') => {
                    self.buf.undo();
                }
                Key::Ctrl('y') | Key::Ctrl('Z') => {
                    self.buf.redo();
                }
                Key::Ctrl('c') => {
                    if let Some(t) = self.buf.selected_text() {
                        crate::ui::clipboard::copy(&t);
                    }
                }
                Key::Ctrl('x') => {
                    if let Ok(Some(t)) = self.buf.cut() {
                        crate::ui::clipboard::copy(&t);
                    }
                }
                _ => return false,
            },
            Event::Paste(t) => {
                if self.buf.byte_len() + t.len() < MAX_CHARS {
                    let _ = self.buf.insert(&t.replace('\r', ""));
                }
            }
            Event::Scroll(d) => {
                self.scroll = (self.scroll as i32 - d * 3).clamp(0, self.buf.line_count().saturating_sub(1) as i32) as usize;
            }
            _ => return false,
        }
        true
    }

    /// A press at (x, y) pixels inside the text area.
    pub fn click(&mut self, x: f32, y: f32, extend: bool) {
        let (cw, lh, _, _) = self.geo;
        let line = (self.scroll + (y / lh).floor().max(0.0) as usize).min(self.buf.line_count().saturating_sub(1));
        let col = self.hscroll + ((x - 8.0) / cw).round().max(0.0) as usize;
        self.buf.set_cursor((line, col), extend);
    }

    pub fn draw(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, tick: u64, placeholder: &str) {
        fx::field_frame(g, r, s, focused, crate::fx::CYAN);
        let size = 13.0 * s;
        let lh = g.line_height(size, Face::Mono);
        let cw = g.text_width(size, "M", Face::Mono).max(4.0);
        let inner = R::new(r.x + 2.0, r.y + 4.0 * s, r.w - 4.0, r.h - 8.0 * s);
        let rows = ((inner.h / lh).floor() as usize).max(1);
        let cols = (((inner.w - 16.0 * s) / cw).floor() as usize).max(4);
        self.geo = (cw, lh, cols, rows);
        let (cl, cc) = self.buf.cursor;
        if cl < self.scroll {
            self.scroll = cl;
        } else if cl >= self.scroll + rows {
            self.scroll = cl + 1 - rows;
        }
        if cc < self.hscroll {
            self.hscroll = cc;
        } else if cc >= self.hscroll + cols {
            self.hscroll = cc + 1 - cols;
        }
        self.scroll = self.scroll.min(self.buf.line_count().saturating_sub(1));
        if self.buf.byte_len() == 0 && !focused {
            g.text(inner.x + 8.0, inner.y, size, DIM, placeholder, Face::Mono);
        }
        let sel = self.buf.selection();
        g.clipped(inner, |g| {
            for i in 0..rows {
                let li = self.scroll + i;
                if li >= self.buf.line_count() {
                    break;
                }
                let y = inner.y + i as f32 * lh;
                let line = self.buf.line(li);
                let chars: Vec<char> = line.chars().collect();
                if let Some((a, b)) = sel {
                    if li >= a.0 && li <= b.0 {
                        let from = if li == a.0 { a.1 } else { 0 };
                        let to = if li == b.0 { b.1 } else { chars.len() + 1 };
                        let (x0, x1) = (from.saturating_sub(self.hscroll), to.saturating_sub(self.hscroll));
                        g.rect(inner.x + 8.0 + x0 as f32 * cw, y, ((x1 - x0) as f32 * cw).max(3.0), lh, with_alpha(crate::fx::CYAN, 70));
                    }
                }
                let shown: String = chars.iter().skip(self.hscroll).take(cols).collect();
                g.text(inner.x + 8.0, y, size, TEXT, &shown, Face::Mono);
                if focused && li == cl && super::blink(tick) && super::caret_allowed() {
                    let x = inner.x + 8.0 + (cc - self.hscroll) as f32 * cw;
                    g.rect(x, y, 1.6 * s, lh, crate::fx::CYAN);
                }
            }
        });
        g.hit(r, Event::Area(self.area), "query");
        if self.buf.line_count() > rows {
            fx::scrollbar(g, R::new(r.x + r.w - 8.0 * s, r.y + 4.0 * s, 5.0 * s, r.h - 8.0 * s), s, self.scroll, rows, self.buf.line_count(), crate::fx::CYAN);
        }
        let _ = (MUTED, rgb(0, 0, 0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_lines_selection_and_undo() {
        let mut q = QueryBox::new(1);
        q.handle(&Event::Key(Key::Char('S')));
        q.handle(&Event::Key(Key::Char('1')));
        q.handle(&Event::Key(Key::Enter));
        q.handle(&Event::Key(Key::Char('x')));
        assert_eq!(q.text(), "S1\nx");
        q.handle(&Event::Key(Key::Ctrl('a')));
        q.handle(&Event::Key(Key::Backspace));
        assert_eq!(q.text(), "");
        q.handle(&Event::Key(Key::Ctrl('z')));
        assert_eq!(q.text(), "S1\nx");
        q.handle(&Event::Paste("a\r\nb".into()));
        assert!(q.text().ends_with("a\nb"));
        assert!(!q.handle(&Event::Key(Key::Esc)), "Esc is the owner's");
        q.set_text(&"x".repeat(MAX_CHARS + 50));
        assert_eq!(q.text().len(), MAX_CHARS);
        q.handle(&Event::Key(Key::Char('y')));
        assert_eq!(q.text().len(), MAX_CHARS, "the limit holds");
    }
}
