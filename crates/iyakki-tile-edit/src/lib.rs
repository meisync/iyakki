//! The code editor as a tile (F-WEB-54): a buffer on the same engine as the EDIT tile (`iyakki-edit`: cursor, undo and redo, line breaks,
//! indentation, brackets) and, now, FILES. A path line names a file of the person's own folders; Open reads it through the service domain
//! `files` and Save writes it back. The service answers with the person's own `read_file` and `write_file` tools (their folders, their
//! write rules, the tool's refusals and audit lines): a write is a tool that asks first, so Save shows an approval card in the approvals
//! list and the tile keeps asking, every two seconds, until the person has answered (a yes saves once, a no is said). Typing arrives as
//! text events, the arrows, Backspace, Delete, Enter and Tab as keys, Ctrl+Z and Ctrl+Y undo and redo, Ctrl+S saves; a click on a line
//! puts the cursor at its end. Not here: the assistant's proposals and diffs, search, syntax colours, pointer selection, the clipboard,
//! folders, and files larger than 12 KiB.
use iyakki_edit::buffer::{Buffer, Move};
use iyakki_edit::{highlight_line, lang_for_path, HlState, Lang, TokenKind};
use iyakki_tile_kit::abi::{line_height, measure_text, Color, DisplayList, Event, Op, Rect};
use iyakki_tile_kit::{button, call_json, clip, palette, text, App, Palette, Source};
use serde_json::json;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
pub const TILE_WASM: &[u8] = include_bytes!("../tile.wasm");

/// The colour of a kind of token in this palette (the colours follow the theme: a light theme gets darker inks).
fn token_color(k: TokenKind, p: &Palette) -> Color {
    let dark = p.bg.0 < 128;
    match (k, dark) {
        (TokenKind::Keyword, true) => Color::rgb(197, 134, 192),
        (TokenKind::Keyword, false) => Color::rgb(140, 40, 150),
        (TokenKind::String, true) => Color::rgb(206, 145, 120),
        (TokenKind::String, false) => Color::rgb(160, 70, 30),
        (TokenKind::Comment, true) => Color::rgb(106, 153, 85),
        (TokenKind::Comment, false) => Color::rgb(60, 110, 50),
        (TokenKind::Number | TokenKind::Constant, true) => Color::rgb(181, 206, 168),
        (TokenKind::Number | TokenKind::Constant, false) => Color::rgb(20, 120, 90),
        (TokenKind::Type, true) => Color::rgb(78, 201, 176),
        (TokenKind::Type, false) => Color::rgb(20, 120, 130),
        (TokenKind::Function | TokenKind::Macro, true) => Color::rgb(220, 220, 170),
        (TokenKind::Function | TokenKind::Macro, false) => Color::rgb(110, 90, 20),
        (TokenKind::Attribute | TokenKind::Key, true) => Color::rgb(156, 220, 254),
        (TokenKind::Attribute | TokenKind::Key, false) => Color::rgb(30, 90, 170),
        (TokenKind::Heading, true) => Color::rgb(86, 156, 214),
        (TokenKind::Heading, false) => Color::rgb(20, 70, 160),
        (TokenKind::Variable, _) => p.fg,
    }
}
pub const CAPS: [&str; 2] = ["files.read", "files.write"];

/// What a tile can read or save of one file (the service refuses more).
pub const MAX_BYTES: usize = 12 * 1024;
const TOP: i32 = 60;
const SIZE: u32 = 13;
const GUTTER: i32 = 34;
const RETRY_MS: u32 = 2000;

pub struct EditTile<S: Source> {
    src: S,
    buf: Buffer,
    size: (i32, i32),
    dark: bool,
    top: usize,
    note: String,
    path: String,
    path_focus: bool,
    /// Find: the text being looked for and whether the keys go to it (Ctrl+F or the Find button); Enter selects the next match, Shift+Enter the one before.
    find: String,
    find_focus: bool,
    /// The text of a save that is waiting for the person's answer, asked again until answered.
    pending: Option<String>,
    saved: String,
    since: u32,
    /// The primary button is down in the text: moving the pointer extends the selection.
    dragging: bool,
    /// A press already placed the cursor: the click that follows (`Hit`) must not move it to the end of the line.
    placed: bool,
}

impl<S: Source> EditTile<S> {
    pub fn new(src: S) -> Self {
        EditTile { src, buf: Buffer::new(), size: (480, 360), dark: false, top: 0, note: String::new(), path: String::new(), path_focus: false, find: String::new(), find_focus: false, pending: None, saved: String::new(), since: 0, dragging: false, placed: false }
    }

    pub fn source(&mut self) -> &mut S {
        &mut self.src
    }

    pub fn text(&self) -> String {
        self.buf.text()
    }

    pub fn cursor(&self) -> (usize, usize) {
        self.buf.cursor
    }

    pub fn note(&self) -> &str {
        &self.note
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// The place in the text under a point of the surface: the line by its row, the column by the nearest gap between letters (the
    /// font is monospaced). `clamp_rows` keeps a point above or below the text on its first or last visible line (for a drag).
    fn pos_at(&self, x: i32, y: i32, clamp_rows: bool) -> Option<(usize, usize)> {
        let lh = line_height(SIZE);
        let adv = measure_text(0, SIZE, "x").max(1);
        let mut row = (y - TOP).div_euclid(lh);
        if clamp_rows {
            row = row.clamp(0, self.rows() as i32 - 1);
        } else if row < 0 || row >= self.rows() as i32 {
            return None;
        }
        let line = (self.top + row as usize).min(self.buf.line_count().saturating_sub(1));
        if !clamp_rows && self.top + row as usize >= self.buf.line_count() {
            return None;
        }
        let len = self.buf.line(line).chars().count() as i32;
        let col = ((x - GUTTER + adv / 2).div_euclid(adv)).clamp(0, len);
        Some((line, col as usize))
    }

    fn rows(&self) -> usize {
        (((self.size.1 - TOP - 20) / line_height(SIZE)).max(1)) as usize
    }

    fn keep_cursor_visible(&mut self) {
        let line = self.buf.cursor.0;
        if line < self.top {
            self.top = line;
        } else if line >= self.top + self.rows() {
            self.top = line + 1 - self.rows();
        }
    }

    /// Select the next (or previous) match of the find text, wrapping around, and say how many there are.
    fn find_step(&mut self, forward: bool) -> bool {
        if self.find.is_empty() {
            self.note = "Type what to find.".into();
            return true;
        }
        let all = self.buf.find_all(&self.find, false).len();
        if all == 0 || !self.buf.select_next(&self.find, false, forward) {
            self.note = format!("No match for {}.", clip(&self.find, 120, 11));
            return true;
        }
        let at = self.buf.selection().map_or(0, |(a, _)| self.buf.find_all(&self.find, false).iter().position(|(x, _)| *x == a).map_or(0, |i| i + 1));
        self.note = format!("Match {at} of {all}");
        self.keep_cursor_visible();
        true
    }

    fn insert(&mut self, s: &str) {
        if self.find_focus {
            for c in s.chars().filter(|c| !c.is_control()) {
                if self.find.chars().count() < 100 {
                    self.find.push(c);
                }
            }
            return;
        }
        if self.path_focus {
            for c in s.chars().filter(|c| !c.is_control()) {
                if self.path.len() < 400 {
                    self.path.push(c);
                }
            }
            return;
        }
        if self.buf.byte_len() + s.len() > MAX_BYTES {
            self.note = "The text is full (12 KiB).".into();
            return;
        }
        let clean: String = s.chars().filter(|c| !c.is_control()).collect();
        // one or two characters are typing (brackets and quotes close themselves); more is a paste, put in as one edit
        let done = if clean.chars().count() <= 2 { clean.chars().try_for_each(|c| self.buf.type_char(c)) } else { self.buf.insert(&clean) };
        if done.is_err() {
            self.note = "That could not be typed.".into();
            return;
        }
        self.note.clear();
    }

    /// Read the file named in the path line into the buffer.
    fn open(&mut self) -> bool {
        self.path_focus = false;
        self.pending = None;
        if self.path.trim().is_empty() {
            self.note = "Type a file name first.".into();
            return true;
        }
        match call_json(&mut self.src, "files", "read", &json!({"path": self.path.trim()}).to_string()) {
            Ok(v) if v["ok"].as_bool() == Some(true) => {
                let t = v["text"].as_str().unwrap_or("").to_string();
                match Buffer::from_text(&t) {
                    Ok(b) => {
                        self.buf = b;
                        self.top = 0;
                        self.saved = self.buf.text();
                        self.note = format!("Opened {} ({} bytes).", self.path.trim(), t.len());
                    }
                    Err(_) => self.note = "That file is not text this editor can open.".into(),
                }
            }
            Ok(v) => self.note = v["error"].as_str().unwrap_or("the file could not be opened").chars().take(200).collect(),
            Err(e) => self.note = e.chars().take(200).collect(),
        }
        true
    }

    /// Ask the service to write the buffer (or the text that is waiting) to the file.
    fn save(&mut self, text: String) -> bool {
        if self.path.trim().is_empty() {
            self.note = "Type a file name first.".into();
            return true;
        }
        self.since = 0;
        match call_json(&mut self.src, "files", "write", &json!({"path": self.path.trim(), "text": text}).to_string()) {
            Ok(v) if v["ok"].as_bool() == Some(true) => {
                self.pending = None;
                self.saved = text;
                self.note = format!("Saved {}.", self.path.trim());
            }
            Ok(v) if v["asked"].as_bool() == Some(true) => {
                self.pending = Some(text);
                self.note = "Waiting for your answer in the approvals list.".into();
            }
            Ok(v) => {
                self.pending = None;
                self.note = v["error"].as_str().unwrap_or("the file could not be saved").chars().take(200).collect();
            }
            Err(e) => {
                self.pending = None;
                self.note = e.chars().take(200).collect();
            }
        }
        true
    }
}

impl<S: Source> EditTile<S> {
    /// The colour runs (start column, end column, colour) of each visible row, or None for a row drawn in one colour: a file whose name
    /// says a language is highlighted by the editing engine (the state of a block comment or a string is carried from the top of the file).
    fn colours(&self, p: &Palette, adv: i32) -> Vec<Option<Vec<(usize, usize, Color)>>> {
        let _ = adv;
        let rows = self.rows();
        let lang = lang_for_path(&self.path);
        if lang == Lang::Plain || self.path.is_empty() {
            return vec![None; rows];
        }
        let last = (self.top + rows).min(self.buf.line_count());
        let mut state = HlState::Normal;
        let mut out = vec![None; rows];
        for n in 0..last {
            let line = self.buf.line(n);
            let (spans, next) = highlight_line(lang, line, state);
            state = next;
            if n < self.top {
                continue;
            }
            let len = line.chars().count();
            let mut runs = Vec::new();
            let mut at = 0usize;
            for sp in spans {
                if sp.start > at {
                    runs.push((at, sp.start, p.fg));
                }
                if sp.end > sp.start {
                    runs.push((sp.start, sp.end.min(len), token_color(sp.kind, p)));
                }
                at = sp.end.max(at);
            }
            if at < len {
                runs.push((at, len, p.fg));
            }
            out[n - self.top] = Some(runs);
        }
        out
    }
}

impl<S: Source> App for EditTile<S> {
    fn event(&mut self, e: &Event) -> bool {
        let changed = match e {
            Event::Resize { w, h, .. } => {
                self.size = (*w, *h);
                true
            }
            Event::Theme { dark, .. } => {
                self.dark = *dark;
                true
            }
            Event::TextInput { text } => {
                self.insert(text);
                true
            }
            Event::Tick { dt_ms } => {
                self.since = self.since.saturating_add(*dt_ms);
                match self.pending.clone() {
                    Some(t) if self.since >= RETRY_MS => self.save(t),
                    _ => false,
                }
            }
            Event::Key { code, mods, down: true } => {
                let ctrl = mods & 2 != 0;
                let shift = mods & 1 != 0;
                if self.find_focus {
                    match *code {
                        8 => {
                            self.find.pop();
                            true
                        }
                        13 => self.find_step(!shift),
                        27 => {
                            self.find_focus = false;
                            self.note.clear();
                            true
                        }
                        _ => false,
                    }
                } else if self.path_focus {
                    match *code {
                        8 => {
                            self.path.pop();
                            true
                        }
                        13 => self.open(),
                        27 => {
                            self.path_focus = false;
                            true
                        }
                        _ => false,
                    }
                } else {
                    match *code {
                        8 => self.buf.backspace_pair().is_ok(),
                        13 => self.buf.newline_pair().is_ok(),
                        9 => self.buf.indent().is_ok(),
                        127 => self.buf.delete().is_ok(),
                        0x100 => {
                            self.buf.move_to(Move::Left, shift);
                            true
                        }
                        0x101 => {
                            self.buf.move_to(Move::Right, shift);
                            true
                        }
                        0x102 => {
                            self.buf.move_to(Move::Up, shift);
                            true
                        }
                        0x103 => {
                            self.buf.move_to(Move::Down, shift);
                            true
                        }
                        c if ctrl && (c == 'z' as u32 || c == 'Z' as u32) => self.buf.undo(),
                        c if ctrl && (c == 'y' as u32 || c == 'Y' as u32) => self.buf.redo(),
                        c if ctrl && (c == 's' as u32 || c == 'S' as u32) => {
                            let t = self.buf.text();
                            self.save(t)
                        }
                        c if ctrl && (c == 'f' as u32 || c == 'F' as u32) => {
                            self.find_focus = true;
                            self.path_focus = false;
                            true
                        }
                        c if ctrl && (c == 'a' as u32 || c == 'A' as u32) => {
                            self.buf.select_all();
                            true
                        }
                        _ => false,
                    }
                }
            }
            Event::Hit { id: 1 } => self.buf.undo(),
            Event::Hit { id: 2 } => self.buf.redo(),
            Event::Hit { id: 3 } => {
                self.buf = Buffer::new();
                self.top = 0;
                self.note.clear();
                self.pending = None;
                self.saved.clear();
                true
            }
            Event::Hit { id: 4 } => self.open(),
            Event::Hit { id: 5 } => {
                let t = self.buf.text();
                self.save(t)
            }
            Event::Hit { id: 6 } => {
                self.path_focus = true;
                self.find_focus = false;
                true
            }
            Event::Hit { id: 7 } => {
                self.find_focus = !self.find_focus;
                self.path_focus = false;
                if !self.find_focus {
                    self.note.clear();
                }
                true
            }
            Event::PointerDown { x, y, button: 0 } if *x >= GUTTER => match self.pos_at(*x, *y, false) {
                Some(pos) => {
                    self.path_focus = false;
                    self.find_focus = false;
                    self.buf.set_cursor(pos, false);
                    self.dragging = true;
                    self.placed = true;
                    true
                }
                None => false,
            },
            Event::PointerMove { x, y } if self.dragging => {
                if let Some(pos) = self.pos_at(*x, *y, true) {
                    self.buf.set_cursor(pos, true);
                }
                true
            }
            Event::PointerUp { x, y, button: 0 } if self.dragging => {
                self.dragging = false;
                if let Some(pos) = self.pos_at(*x, *y, true) {
                    self.buf.set_cursor(pos, true);
                }
                true
            }
            Event::Hit { id } if *id >= 1000 && self.placed => {
                // the press placed the cursor (and a drag may have selected): nothing more to do
                self.placed = false;
                self.path_focus = false;
                self.find_focus = false;
                false
            }
            Event::Hit { id } if *id >= 1000 => {
                self.path_focus = false;
                self.find_focus = false;
                let line = self.top + (*id - 1000) as usize;
                if line < self.buf.line_count() {
                    let end = self.buf.line(line).chars().count();
                    self.buf.set_cursor((line, end), false);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if changed {
            self.keep_cursor_visible();
        }
        changed
    }

    fn frame(&self) -> DisplayList {
        let (w, h) = self.size;
        let p = palette(self.dark);
        let mut l = DisplayList::new();
        l.push(Op::Rect { r: Rect::new(0, 0, w, h), color: p.bg });
        l.push(Op::Rect { r: Rect::new(0, 0, w, 32), color: p.bar });
        let dirty = self.buf.text() != self.saved;
        l.push(text(8, 8, 14, p.fg, if dirty { "EDIT *" } else { "EDIT" }));
        button(&mut l, 3, Rect::new(w - 56, 4, 50, 24), "Clear", &p);
        button(&mut l, 2, Rect::new(w - 106, 4, 46, 24), "Redo", &p);
        button(&mut l, 1, Rect::new(w - 156, 4, 46, 24), "Undo", &p);
        button(&mut l, 5, Rect::new(w - 206, 4, 46, 24), "Save", &p);
        button(&mut l, 4, Rect::new(w - 256, 4, 46, 24), "Open", &p);
        button(&mut l, 7, Rect::new(w - 306, 4, 46, 24), "Find", &p);
        // the path line: click it to type a file name, Enter opens it
        let on_find = self.find_focus;
        l.push(Op::Outline { r: Rect::new(4, 36, w - 8, 20), thickness: 1, color: if self.path_focus || on_find { p.accent } else { p.mute } });
        let shown = if on_find {
            format!("find: {}|", self.find)
        } else if self.path.is_empty() && !self.path_focus {
            "file: click and type a name".to_string()
        } else {
            format!("file: {}{}", self.path, if self.path_focus { "|" } else { "" })
        };
        l.push(text(8, 40, 12, if self.path.is_empty() && !on_find { p.mute } else { p.fg }, &clip(&shown, w - 20, 12)));
        l.push(Op::Hit { id: 6, r: Rect::new(4, 36, w - 8, 20) });
        let lh = line_height(SIZE);
        let adv = measure_text(0, SIZE, "x").max(1);
        l.push(Op::PushClip { r: Rect::new(0, TOP, w, h - TOP - 20) });
        let colours = self.colours(&p, adv);
        for k in 0..self.rows() {
            let n = self.top + k;
            if n >= self.buf.line_count() {
                break;
            }
            let y = TOP + k as i32 * lh;
            if let Some((a, b)) = self.buf.selection() {
                if n >= a.0 && n <= b.0 {
                    let len = self.buf.line(n).chars().count();
                    let (from, to) = (if n == a.0 { a.1 } else { 0 }, if n == b.0 { b.1 } else { len });
                    if to > from {
                        let x0 = GUTTER + from as i32 * adv;
                        l.push(Op::Rect { r: Rect::new(x0, y, ((to - from) as i32 * adv).min(w - x0).max(0), lh), color: Color(p.accent.0, p.accent.1, p.accent.2, 90) });
                    }
                }
            }
            l.push(text(4, y, SIZE, p.mute, &format!("{}", n + 1)));
            match colours.get(k).and_then(|c| c.as_ref()) {
                // a file with a known language: each run of the line in its own colour, placed by column (the tile's font is monospaced)
                Some(runs) => {
                    let line: Vec<char> = self.buf.line(n).chars().collect();
                    for (a, b, color) in runs {
                        let x = GUTTER + *a as i32 * adv;
                        if x >= w - 4 || *a >= line.len() {
                            continue;
                        }
                        let piece: String = line[*a..(*b).min(line.len())].iter().collect();
                        l.push(text(x, y, SIZE, *color, &clip(&piece, w - x - 4, SIZE)));
                    }
                }
                None => l.push(text(GUTTER, y, SIZE, p.fg, &clip(self.buf.line(n), w - GUTTER - 4, SIZE))),
            }
            l.push(Op::Hit { id: 1000 + k as u32, r: Rect::new(GUTTER, y, w - GUTTER, lh) });
            if n == self.buf.cursor.0 && !self.path_focus {
                let x = GUTTER + self.buf.cursor.1 as i32 * adv;
                if x < w - 2 {
                    l.push(Op::Rect { r: Rect::new(x, y, 2, lh), color: p.accent });
                }
            }
        }
        l.push(Op::PopClip);
        let status = if self.note.is_empty() { format!("line {}, column {}", self.buf.cursor.0 + 1, self.buf.cursor.1 + 1) } else { self.note.clone() };
        let c = if self.note.starts_with("Waiting") { Color(200, 130, 20, 255) } else if self.note.is_empty() || self.note.starts_with("Saved") || self.note.starts_with("Opened") || self.note.starts_with("Match") { p.mute } else { p.bad };
        l.push(text(8, h - 16, 11, c, &clip(&status, w - 16, 11)));
        l
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    pub struct Fake {
        pub calls: Vec<(String, String, String)>,
        pub replies: VecDeque<Result<String, String>>,
    }
    impl Source for Fake {
        fn call(&mut self, d: &str, m: &str, j: &str) -> Result<String, String> {
            self.calls.push((d.into(), m.into(), j.into()));
            self.replies.pop_front().unwrap_or_else(|| Err("no more replies".into()))
        }
    }

    fn tile(r: Vec<Result<String, String>>) -> EditTile<Fake> {
        EditTile::new(Fake { replies: r.into(), ..Default::default() })
    }

    fn typed<S: Source>(t: &mut EditTile<S>, s: &str) {
        t.event(&Event::TextInput { text: s.into() });
    }

    fn key<S: Source>(t: &mut EditTile<S>, code: u32, mods: u8) -> bool {
        t.event(&Event::Key { code, mods, down: true })
    }

    fn texts(l: &DisplayList) -> Vec<String> {
        l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
    }

    fn ok(t: &str) -> Result<String, String> {
        Ok(json!({"ok": true, "text": t}).to_string())
    }

    #[test]
    fn a_press_places_the_cursor_where_it_lands_and_a_drag_selects() {
        let mut t = tile(vec![]);
        t.event(&Event::Resize { w: 300, h: 200, scale_pct: 100 });
        t.event(&Event::TextInput { text: "hello world".into() });
        t.event(&Event::Key { code: 13, mods: 0, down: true });
        t.event(&Event::TextInput { text: "second line".into() });
        let (lh, adv) = (line_height(SIZE), measure_text(0, SIZE, "x"));
        let at = |line: i32, col: i32| (GUTTER + col * adv + 1, TOP + line * lh + lh / 2);
        // a click in the first line between two letters puts the cursor there, not at the end of the line
        let (x, y) = at(0, 5);
        t.event(&Event::PointerDown { x, y, button: 0 });
        t.event(&Event::PointerUp { x, y, button: 0 });
        t.event(&Event::Hit { id: 1000 });
        assert_eq!(t.buf.cursor, (0, 5));
        assert_eq!(t.buf.selection(), None);
        // a drag from the 3rd letter of the first line to the 4th of the second selects what lies between
        let (x0, y0) = at(0, 2);
        let (x1, y1) = at(1, 4);
        t.event(&Event::PointerDown { x: x0, y: y0, button: 0 });
        t.event(&Event::PointerMove { x: x1, y: y1 });
        t.event(&Event::PointerUp { x: x1, y: y1, button: 0 });
        t.event(&Event::Hit { id: 1001 });
        assert_eq!(t.buf.selection(), Some(((0, 2), (1, 4))));
        assert_eq!(t.buf.selected_text().as_deref(), Some("llo world\nseco"));
        // the drawn selection follows; a press past the end of a line stops at its end, a press on the gutter does nothing
        let (x, y) = at(0, 40);
        t.event(&Event::PointerDown { x, y, button: 0 });
        t.event(&Event::PointerUp { x, y, button: 0 });
        assert_eq!(t.buf.cursor, (0, 11));
        assert!(!t.event(&Event::PointerDown { x: 5, y, button: 0 }), "the gutter is not text");
        // a press on a row below the text does nothing, a right button never places the cursor
        assert!(!t.event(&Event::PointerDown { x: GUTTER + 4, y: TOP + 10 * lh, button: 0 }));
        assert!(!t.event(&Event::PointerDown { x: GUTTER + 4, y: TOP + 2, button: 2 }));
        // a move without a press changes nothing
        assert!(!t.event(&Event::PointerMove { x: 50, y: 70 }));
    }

    #[test]
    fn typing_enter_backspace_and_the_arrows_edit_the_text_like_the_edit_tile() {
        let mut t = tile(vec![]);
        typed(&mut t, "fn main() {");
        key(&mut t, 13, 0);
        typed(&mut t, "x");
        assert!(t.text().starts_with("fn main() {"), "{:?}", t.text());
        assert_eq!(t.cursor().0, 1);
        key(&mut t, 8, 0);
        key(&mut t, 0x102, 0);
        assert_eq!(t.cursor().0, 0);
        key(&mut t, 0x103, 0);
        assert_eq!(t.cursor().0, 1);
        let x = texts(&t.frame());
        assert!(x.contains(&"1".to_string()) && x.contains(&"2".to_string()) && x.iter().any(|s| s.starts_with("fn main()")), "{x:?}");
        assert!(t.source().calls.is_empty(), "no call for plain editing");
    }

    #[test]
    fn undo_and_redo_by_key_and_by_button_agree() {
        let mut t = tile(vec![]);
        typed(&mut t, "hello");
        let before = t.text();
        assert!(key(&mut t, 'z' as u32, 2));
        assert_ne!(t.text(), before);
        assert!(key(&mut t, 'y' as u32, 2));
        assert_eq!(t.text(), before);
        assert!(t.event(&Event::Hit { id: 1 }));
        assert!(t.event(&Event::Hit { id: 2 }));
        assert_eq!(t.text(), before);
        assert!(!t.event(&Event::Hit { id: 2 }), "nothing more to redo: no change");
        assert!(t.event(&Event::Hit { id: 3 }));
        assert_eq!(t.text(), "");
    }

    #[test]
    fn a_click_on_a_line_puts_the_cursor_at_its_end_and_a_click_below_the_text_does_nothing() {
        let mut t = tile(vec![]);
        typed(&mut t, "one");
        key(&mut t, 13, 0);
        typed(&mut t, "two");
        assert!(t.event(&Event::Hit { id: 1000 }));
        assert_eq!(t.cursor(), (0, 3));
        assert!(!t.event(&Event::Hit { id: 1010 }));
    }

    #[test]
    fn long_text_scrolls_to_keep_the_cursor_in_view_and_long_lines_are_clipped() {
        let mut t = tile(vec![]);
        t.event(&Event::Resize { w: 200, h: 160, scale_pct: 100 });
        for i in 0..40 {
            typed(&mut t, &format!("line {i} {}", "y".repeat(100)));
            key(&mut t, 13, 0);
        }
        let l = t.frame();
        let x = texts(&l);
        assert!(x.iter().any(|s| s.starts_with("line 39") || s == "41"), "{x:?}");
        for o in &l.ops {
            if let Op::Text { x, size, text, .. } = o {
                assert!(x + measure_text(0, *size, text) <= 200, "{text}");
            }
        }
        for _ in 0..50 {
            key(&mut t, 0x102, 0);
        }
        assert!(texts(&t.frame()).iter().any(|s| s.starts_with("line 0")), "back at the top");
    }

    #[test]
    fn the_text_has_a_limit_control_characters_are_dropped_and_hostile_input_never_panics() {
        let mut t = tile(vec![]);
        typed(&mut t, "a\u{0}b\u{7}c");
        assert_eq!(t.text().replace('\n', ""), "abc");
        let big = "z".repeat(1000);
        for _ in 0..30 {
            typed(&mut t, &big);
        }
        assert!(t.text().len() <= MAX_BYTES + 4, "{}", t.text().len());
        assert!(texts(&t.frame()).iter().any(|s| s.contains("full")));
        for code in [0u32, 1, 127, 0x105, 0xffff_ffff] {
            key(&mut t, code, 0xff);
        }
        for id in [0u32, 99, 5000, u32::MAX] {
            t.event(&Event::Hit { id });
        }
        assert!(t.frame().encode().is_ok());
    }

    #[test]
    fn a_file_is_opened_by_its_name_and_a_refusal_is_said_in_the_service_s_words() {
        let mut t = tile(vec![ok("fn a() {}\n"), Ok(json!({"ok": false, "error": "no such file"}).to_string())]);
        assert!(t.event(&Event::Hit { id: 6 }));
        typed(&mut t, "/home/ann/a.rs");
        assert_eq!(t.path(), "/home/ann/a.rs");
        key(&mut t, 13, 0);
        assert_eq!(t.source().calls[0], ("files".to_string(), "read".to_string(), r#"{"path":"/home/ann/a.rs"}"#.to_string()));
        assert_eq!(t.text(), "fn a() {}\n");
        assert!(t.note().starts_with("Opened /home/ann/a.rs"), "{}", t.note());
        assert!(!texts(&t.frame()).contains(&"EDIT *".to_string()), "just opened is not changed");
        typed(&mut t, "x");
        assert!(texts(&t.frame()).contains(&"EDIT *".to_string()), "a change shows");
        t.event(&Event::Hit { id: 6 });
        t.event(&Event::Hit { id: 4 });
        assert_eq!(t.note(), "no such file");
        assert!(texts(&t.frame()).contains(&"no such file".to_string()));
    }

    #[test]
    fn a_save_waits_for_the_persons_answer_asks_again_every_two_seconds_and_writes_once_on_a_yes() {
        let asked = || Ok(json!({"ok": false, "asked": true}).to_string());
        let mut t = tile(vec![asked(), asked(), ok("wrote /home/ann/a.rs")]);
        t.event(&Event::Hit { id: 6 });
        typed(&mut t, "/home/ann/a.rs");
        t.event(&Event::Hit { id: 1000 }); // leaves the path line
        typed(&mut t, "let x = 1;");
        assert!(key(&mut t, 's' as u32, 2));
        assert_eq!(t.source().calls.len(), 1);
        assert!(t.note().starts_with("Waiting for your answer"), "{}", t.note());
        assert!(!t.event(&Event::Tick { dt_ms: 1500 }), "not before two seconds");
        assert!(t.event(&Event::Tick { dt_ms: 600 }));
        assert_eq!(t.source().calls.len(), 2);
        // the text the question was asked about is the text that is asked again, even if the buffer changed meanwhile
        let first = t.source().calls[0].2.clone();
        typed(&mut t, "more");
        assert!(t.event(&Event::Tick { dt_ms: 2000 }));
        assert_eq!(t.source().calls[2].2, first);
        assert!(t.note().starts_with("Saved /home/ann/a.rs"), "{}", t.note());
        assert!(!t.event(&Event::Tick { dt_ms: 5000 }), "nothing is asked once it is saved");
        assert_eq!(t.source().calls.len(), 3);
    }

    #[test]
    fn a_no_and_an_error_end_the_wait_and_an_empty_name_asks_nothing() {
        let mut t = tile(vec![Ok(json!({"ok": false, "asked": true}).to_string()), Ok(json!({"ok": false, "error": "you said no"}).to_string())]);
        key(&mut t, 's' as u32, 2);
        assert_eq!(t.note(), "Type a file name first.");
        assert!(t.source().calls.is_empty());
        t.event(&Event::Hit { id: 6 });
        typed(&mut t, "/etc/passwd");
        t.event(&Event::Hit { id: 1000 });
        t.event(&Event::Hit { id: 5 });
        t.event(&Event::Tick { dt_ms: 2500 });
        assert_eq!(t.note(), "you said no");
        assert!(!t.event(&Event::Tick { dt_ms: 5000 }), "no more asking after a no");
        let mut t = tile(vec![Err("this tile is not allowed to do that".into())]);
        t.event(&Event::Hit { id: 6 });
        typed(&mut t, "/a");
        t.event(&Event::Hit { id: 1000 });
        t.event(&Event::Hit { id: 5 });
        assert_eq!(t.note(), "this tile is not allowed to do that");
        for bad in ["", "null", "[]", "{\"ok\":5}", "{\"text\":1}"] {
            let mut t = tile(vec![Ok(bad.into())]);
            t.event(&Event::Hit { id: 6 });
            typed(&mut t, "/a");
            key(&mut t, 13, 0);
            assert!(t.frame().encode().is_ok(), "{bad}");
        }
    }

    #[test]
    fn a_file_with_a_language_name_is_drawn_in_colours_and_a_scratch_buffer_or_a_text_file_is_not() {
        let mut t = tile(vec![]);
        t.event(&Event::Resize { w: 480, h: 360, scale_pct: 100 });
        for (i, line) in ["// note", "fn main() { let x = \"hi\"; }", "/* open", "still */ let y = 3;"].iter().enumerate() {
            if i > 0 {
                key(&mut t, 13, 0);
            }
            typed(&mut t, line);
        }
        let colours = |t: &EditTile<Fake>| -> std::collections::BTreeSet<[u8; 3]> {
            t.frame().ops.iter().filter_map(|o| if let Op::Text { color, y, .. } = o { if *y >= 56 { Some([color.0, color.1, color.2]) } else { None } } else { None }).collect()
        };
        let plain = colours(&t);
        t.path = "scratch.txt".into();
        assert_eq!(colours(&t), plain, "no language by the name: one ink for the text, as before");
        assert!(texts_of(&t).iter().any(|s| s.contains("fn main()")), "and the line is drawn whole");
        t.path = "main.rs".into();
        let coloured = colours(&t);
        assert!(coloured.len() >= plain.len() + 3, "keywords, strings, comments and numbers get their own inks: {coloured:?} against {plain:?}");
        let words = texts_of(&t);
        for w in ["// note", "fn", "main", "\"hi\"", "let", "3"] {
            assert!(words.iter().any(|s| s == w), "{w} is its own run: {words:?}");
        }
        // a block comment that began on an earlier line keeps its colour on the next one
        let comment = |t: &EditTile<Fake>, what: &str| t.frame().ops.iter().find_map(|o| if let Op::Text { text, color, .. } = o { (text == what).then_some([color.0, color.1, color.2]) } else { None });
        assert_eq!(comment(&t, "still */"), comment(&t, "// note"), "a comment line and the end of a block comment have the comment ink");
        // the runs sit where the characters sit: the cursor column and the run columns agree
        let x_of = |what: &str| t.frame().ops.iter().find_map(|o| if let Op::Text { x, text, .. } = o { (text == what).then_some(*x) } else { None });
        let adv = measure_text(0, SIZE, "x").max(1);
        assert_eq!(x_of("let"), Some(GUTTER + 12 * adv), "a run is placed at its column");
        // a long file name keeps the same colours and typing keeps working
        typed(&mut t, "z");
        assert!(!texts_of(&t).is_empty());
    }

    fn texts_of<S: Source>(t: &EditTile<S>) -> Vec<String> {
        t.frame().ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
    }

    #[test]
    fn find_selects_each_match_in_turn_wraps_says_how_many_and_leaves_the_text_alone() {
        let mut t = tile(vec![]);
        t.event(&Event::Resize { w: 480, h: 360, scale_pct: 100 });
        for (i, line) in ["the cat sat", "on the mat", "The end"].iter().enumerate() {
            if i > 0 {
                key(&mut t, 13, 0);
            }
            typed(&mut t, line);
        }
        let before = t.buf.text();
        assert!(key(&mut t, 'f' as u32, 2), "Ctrl+F starts find");
        assert!(texts_of(&t).iter().any(|s| s == "find: |"));
        typed(&mut t, "THE");
        assert!(texts_of(&t).iter().any(|s| s == "find: THE|"), "typing goes to the find line, not to the text: {:?}", texts_of(&t));
        assert_eq!(t.buf.text(), before);
        // Enter: the next match from the cursor (wrapping), case does not matter
        key(&mut t, 13, 0);
        assert_eq!(t.buf.selection().map(|(a, b)| (a, b)), Some(((0, 0), (0, 3))), "first match from the top after wrapping: {:?}", t.buf.selection());
        assert!(texts_of(&t).contains(&"Match 1 of 3".to_string()), "{:?}", texts_of(&t));
        key(&mut t, 13, 0);
        assert_eq!(t.buf.selection(), Some(((1, 3), (1, 6))));
        key(&mut t, 13, 0);
        key(&mut t, 13, 0);
        assert_eq!(t.buf.selection(), Some(((0, 0), (0, 3))), "it wraps");
        key(&mut t, 13, 1);
        assert_eq!(t.buf.selection(), Some(((2, 0), (2, 3))), "Shift+Enter goes back, wrapping");
        // the match is drawn under its text
        let rects = |t: &EditTile<Fake>| t.frame().ops.iter().filter(|o| matches!(o, Op::Rect { color, .. } if color.3 == 90)).count();
        assert_eq!(rects(&t), 1);
        // no match, an empty find line, and Backspace
        for _ in 0..3 {
            key(&mut t, 8, 0);
        }
        key(&mut t, 13, 0);
        assert!(texts_of(&t).contains(&"Type what to find.".to_string()));
        typed(&mut t, "zzz");
        key(&mut t, 13, 0);
        assert!(texts_of(&t).iter().any(|s| s.starts_with("No match for zzz")), "{:?}", texts_of(&t));
        // Esc leaves find: typing is typing again, and nothing was ever changed by find
        key(&mut t, 27, 0);
        typed(&mut t, "!");
        assert!(t.buf.text().ends_with('!') || t.buf.text().contains('!'));
        // the button toggles it too, and a click on the text leaves it
        t.event(&Event::Hit { id: 7 });
        assert!(texts_of(&t).iter().any(|s| s.starts_with("find: ")));
        t.event(&Event::Hit { id: 1000 });
        typed(&mut t, "?");
        assert!(t.buf.text().contains('?'));
        assert!(t.frame().encode().is_ok());
    }
}
