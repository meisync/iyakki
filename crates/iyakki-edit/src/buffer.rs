//! Text buffer: lines of text, a cursor with an optional selection, undo/redo and search.
//!
//! Columns are counted in `char`s. Every change goes through [`Buffer::replace`], so undo, redo and the
//! "what changed" bookkeeping used by the highlighter cache have exactly one code path.

/// (line, column) with both zero-based; the column counts characters.
pub type Pos = (usize, usize);

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_UNDO: usize = 2000;

#[derive(Debug, PartialEq, Eq)]
pub enum EditError {
    TooLarge,
    Binary,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::TooLarge => write!(f, "the file is too large to edit here (limit 2 MiB)"),
            EditError::Binary => write!(f, "this looks like a binary file"),
        }
    }
}

impl std::error::Error for EditError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    /// First non-blank character, then column 0 on a second press.
    Home,
    End,
    PageUp(usize),
    PageDown(usize),
    DocStart,
    DocEnd,
    WordLeft,
    WordRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Typing,
    Backspace,
    Delete,
    Other,
}

#[derive(Clone, Debug)]
struct Edit {
    start: Pos,
    removed: String,
    inserted: String,
    cursor_before: Pos,
    anchor_before: Option<Pos>,
    cursor_after: Pos,
    kind: Kind,
}

#[derive(Clone, Debug)]
pub struct Buffer {
    lines: Vec<String>,
    pub cursor: Pos,
    anchor: Option<Pos>,
    want_col: usize,
    crlf: bool,
    trailing_newline: bool,
    pub tab_width: usize,
    bytes: usize,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    revision: u64,
    /// Undo depth at the last save (`None`: the saved state can no longer be reached).
    saved_depth: Option<usize>,
    /// The file was exactly one newline (a single blank line), not empty.
    blank_line_file: bool,
    dirty_from: Option<usize>,
}

fn clen(s: &str) -> usize {
    s.chars().count()
}

fn byte_of(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map_or(s.len(), |(i, _)| i)
}

/// Where the cursor ends up after `text` is inserted at `start`.
fn end_of(start: Pos, text: &str) -> Pos {
    let nl = text.matches('\n').count();
    if nl == 0 {
        (start.0, start.1 + clen(text))
    } else {
        (start.0 + nl, clen(text.rsplit('\n').next().unwrap_or("")))
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::new()
    }
}

impl Buffer {
    pub fn new() -> Buffer {
        Buffer { lines: vec![String::new()], cursor: (0, 0), anchor: None, want_col: 0, crlf: false, trailing_newline: true, tab_width: 4, bytes: 0, undo: vec![], redo: vec![], revision: 0, saved_depth: Some(0), blank_line_file: false, dirty_from: None }
    }

    pub fn from_text(text: &str) -> Result<Buffer, EditError> {
        if text.len() > MAX_BYTES {
            return Err(EditError::TooLarge);
        }
        if text.contains('\0') {
            return Err(EditError::Binary);
        }
        let crlf = text.contains("\r\n");
        let trailing_newline = text.is_empty() || text.ends_with('\n');
        let norm = text.replace("\r\n", "\n");
        let body = norm.strip_suffix('\n').unwrap_or(&norm);
        let lines: Vec<String> = body.split('\n').map(|l| l.replace('\r', "")).collect();
        let mut b = Buffer::new();
        b.blank_line_file = norm == "\n";
        b.bytes = lines.iter().map(|l| l.len()).sum::<usize>() + lines.len().saturating_sub(1);
        b.lines = lines;
        b.crlf = crlf;
        b.trailing_newline = trailing_newline;
        Ok(b)
    }

    /// The text as it would be saved (original line endings and final newline preserved).
    pub fn text(&self) -> String {
        let sep = if self.crlf { "\r\n" } else { "\n" };
        let mut s = self.lines.join(sep);
        let single_blank = self.lines.len() == 1 && self.lines[0].is_empty();
        if (self.trailing_newline && !single_blank) || (single_blank && self.blank_line_file) {
            s.push_str(sep);
        }
        s
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
    pub fn line(&self, i: usize) -> &str {
        self.lines.get(i).map_or("", String::as_str)
    }
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
    pub fn byte_len(&self) -> usize {
        self.bytes
    }
    pub fn uses_crlf(&self) -> bool {
        self.crlf
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn is_dirty(&self) -> bool {
        self.saved_depth != Some(self.undo.len())
    }
    pub fn mark_saved(&mut self) {
        self.saved_depth = Some(self.undo.len());
    }
    /// First line changed since the last call (for highlighter invalidation).
    pub fn take_dirty(&mut self) -> Option<usize> {
        self.dirty_from.take()
    }

    // ------------------------------------------------------------------ positions and selection

    fn clamp(&self, p: Pos) -> Pos {
        let l = p.0.min(self.lines.len() - 1);
        (l, p.1.min(clen(&self.lines[l])))
    }

    pub fn set_cursor(&mut self, p: Pos, extend: bool) {
        let p = self.clamp(p);
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = p;
        self.want_col = p.1;
        if self.anchor == Some(self.cursor) {
            self.anchor = None;
        }
    }

    pub fn goto_line(&mut self, line1: usize) {
        self.set_cursor((line1.saturating_sub(1), 0), false);
    }

    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        let c = self.cursor;
        if a == c { None } else if a < c { Some((a, c)) } else { Some((c, a)) }
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        let last = self.lines.len() - 1;
        self.cursor = (last, clen(&self.lines[last]));
        self.want_col = self.cursor.1;
        if self.anchor == Some(self.cursor) {
            self.anchor = None;
        }
    }

    pub fn select_range(&mut self, a: Pos, b: Pos) {
        let (a, b) = (self.clamp(a), self.clamp(b));
        self.anchor = if a == b { None } else { Some(a) };
        self.cursor = b;
        self.want_col = b.1;
    }

    fn text_in(&self, a: Pos, b: Pos) -> String {
        if a.0 == b.0 {
            let l = &self.lines[a.0];
            return l[byte_of(l, a.1)..byte_of(l, b.1)].to_string();
        }
        let mut s = self.lines[a.0][byte_of(&self.lines[a.0], a.1)..].to_string();
        for i in a.0 + 1..b.0 {
            s.push('\n');
            s.push_str(&self.lines[i]);
        }
        s.push('\n');
        s.push_str(&self.lines[b.0][..byte_of(&self.lines[b.0], b.1)]);
        s
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|(a, b)| self.text_in(a, b))
    }

    // ------------------------------------------------------------------ movement

    pub fn move_to(&mut self, m: Move, extend: bool) {
        let (l, c) = self.cursor;
        let last = self.lines.len() - 1;
        let mut keep_want = false;
        // without Shift, an arrow first collapses a selection to its edge
        if !extend {
            if let (Some((a, b)), Move::Left | Move::Right) = (self.selection(), m) {
                self.anchor = None;
                self.cursor = if m == Move::Left { a } else { b };
                self.want_col = self.cursor.1;
                return;
            }
        }
        let target = match m {
            Move::Left => {
                if c > 0 { (l, c - 1) } else if l > 0 { (l - 1, clen(&self.lines[l - 1])) } else { (0, 0) }
            }
            Move::Right => {
                if c < clen(&self.lines[l]) { (l, c + 1) } else if l < last { (l + 1, 0) } else { (l, c) }
            }
            Move::Up | Move::Down | Move::PageUp(_) | Move::PageDown(_) => {
                keep_want = true;
                let nl = match m {
                    Move::Up => l.saturating_sub(1),
                    Move::Down => (l + 1).min(last),
                    Move::PageUp(n) => l.saturating_sub(n.max(1)),
                    Move::PageDown(n) => (l + n.max(1)).min(last),
                    _ => l,
                };
                if nl == l && matches!(m, Move::Up) {
                    (0, 0)
                } else if nl == l && matches!(m, Move::Down) {
                    (l, clen(&self.lines[l]))
                } else {
                    (nl, self.want_col.min(clen(&self.lines[nl])))
                }
            }
            Move::Home => {
                let first = self.lines[l].chars().take_while(|c| *c == ' ' || *c == '\t').count();
                (l, if c == first { 0 } else { first })
            }
            Move::End => (l, clen(&self.lines[l])),
            Move::DocStart => (0, 0),
            Move::DocEnd => (last, clen(&self.lines[last])),
            Move::WordLeft => self.word_left(),
            Move::WordRight => self.word_right(),
        };
        let want = self.want_col;
        self.set_cursor(target, extend);
        if keep_want {
            self.want_col = want;
        }
    }

    fn word_left(&self) -> Pos {
        let (mut l, mut c) = self.cursor;
        loop {
            let chars: Vec<char> = self.lines[l].chars().collect();
            while c > 0 && !is_word(chars[c - 1]) {
                c -= 1;
            }
            if c > 0 {
                while c > 0 && is_word(chars[c - 1]) {
                    c -= 1;
                }
                return (l, c);
            }
            if l == 0 {
                return (0, 0);
            }
            l -= 1;
            c = clen(&self.lines[l]);
            if c == 0 {
                return (l, 0);
            }
        }
    }

    fn word_right(&self) -> Pos {
        let (mut l, mut c) = self.cursor;
        let last = self.lines.len() - 1;
        loop {
            let chars: Vec<char> = self.lines[l].chars().collect();
            while c < chars.len() && !is_word(chars[c]) {
                c += 1;
            }
            if c < chars.len() {
                while c < chars.len() && is_word(chars[c]) {
                    c += 1;
                }
                return (l, c);
            }
            if l == last {
                return (l, chars.len());
            }
            l += 1;
            c = 0;
        }
    }

    // ------------------------------------------------------------------ the one mutation primitive

    /// Replace the text between `a` and `b` with `text`, with no history. Returns the end of the insertion.
    fn raw_replace(&mut self, a: Pos, b: Pos, text: &str) -> Pos {
        let head = self.lines[a.0][..byte_of(&self.lines[a.0], a.1)].to_string();
        let tail = self.lines[b.0][byte_of(&self.lines[b.0], b.1)..].to_string();
        let removed_len = self.text_in(a, b).len();
        let joined = format!("{head}{text}{tail}");
        let new: Vec<String> = joined.split('\n').map(String::from).collect();
        self.lines.splice(a.0..=b.0, new);
        self.bytes = self.bytes + text.len() - removed_len;
        self.revision += 1;
        self.dirty_from = Some(self.dirty_from.map_or(a.0, |d| d.min(a.0)));
        end_of(a, text)
    }

    fn replace_with(&mut self, a: Pos, b: Pos, text: &str, kind: Kind) -> Result<(), EditError> {
        let (a, b) = (self.clamp(a), self.clamp(b));
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let removed = self.text_in(a, b);
        if self.bytes + text.len() > MAX_BYTES + removed.len() {
            return Err(EditError::TooLarge);
        }
        let (cursor_before, anchor_before) = (self.cursor, self.anchor);
        let end = self.raw_replace(a, b, &text);
        self.cursor = end;
        self.anchor = None;
        self.want_col = end.1;
        if self.saved_depth.is_some_and(|d| d > self.undo.len()) {
            self.saved_depth = None; // the saved state lived in the redo branch we are discarding
        }
        self.redo.clear();
        // merge runs of typing / backspacing / deleting into one undo step
        let depth = self.undo.len();
        if let Some(last) = self.undo.last_mut() {
            let mergeable = last.kind == kind
                && match kind {
                    Kind::Typing => removed.is_empty() && !text.contains('\n') && last.cursor_after == a && !last.inserted.contains('\n'),
                    Kind::Backspace => text.is_empty() && !removed.contains('\n') && last.cursor_after == b && a.0 == last.start.0,
                    Kind::Delete => text.is_empty() && !removed.contains('\n') && last.start == a,
                    Kind::Other => false,
                };
            if mergeable {
                if self.saved_depth == Some(depth) {
                    self.saved_depth = None; // we are changing the very edit that was saved
                }
                match kind {
                    Kind::Typing => last.inserted.push_str(&text),
                    Kind::Backspace => {
                        last.removed.insert_str(0, &removed);
                        last.start = a;
                    }
                    Kind::Delete => last.removed.push_str(&removed),
                    Kind::Other => {}
                }
                last.cursor_after = end;
                return Ok(());
            }
        }
        self.undo.push(Edit { start: a, removed, inserted: text, cursor_before, anchor_before, cursor_after: end, kind });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
            self.saved_depth = self.saved_depth.and_then(|d| d.checked_sub(1));
        }
        Ok(())
    }

    /// Replace a range (public form, one undo step).
    pub fn replace(&mut self, a: Pos, b: Pos, text: &str) -> Result<(), EditError> {
        self.replace_with(a, b, text, Kind::Other)
    }

    // ------------------------------------------------------------------ editing commands

    fn delete_selection_kind(&mut self, kind: Kind) -> Result<bool, EditError> {
        match self.selection() {
            Some((a, b)) => {
                self.replace_with(a, b, "", if kind == Kind::Typing { Kind::Other } else { kind })?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Type or paste text (replacing the selection). A lone newline auto-indents.
    pub fn insert(&mut self, text: &str) -> Result<(), EditError> {
        if text == "\n" || text == "\r\n" {
            return self.newline();
        }
        let (a, b) = self.selection().unwrap_or((self.cursor, self.cursor));
        let kind = if self.selection().is_none() && !text.contains('\n') && clen(text) == 1 { Kind::Typing } else { Kind::Other };
        self.replace_with(a, b, text, kind)
    }

    pub fn newline(&mut self) -> Result<(), EditError> {
        let (a, b) = self.selection().unwrap_or((self.cursor, self.cursor));
        let line = &self.lines[a.0];
        let upto: String = line.chars().take(a.1).collect();
        let indent: String = upto.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let opener = matches!(upto.trim_end().chars().last(), Some('{' | '(' | '[' | ':'));
        let extra = if opener { " ".repeat(self.tab_width) } else { String::new() };
        self.replace_with(a, b, &format!("\n{indent}{extra}"), Kind::Other)
    }

    pub fn backspace(&mut self) -> Result<(), EditError> {
        if self.delete_selection_kind(Kind::Other)? {
            return Ok(());
        }
        let (l, c) = self.cursor;
        if c > 0 {
            // inside leading indentation, step back to the previous tab stop
            let before: String = self.lines[l].chars().take(c).collect();
            let n = if before.chars().all(|ch| ch == ' ') && c % self.tab_width == 0 { self.tab_width } else { 1 };
            self.replace_with((l, c - n), (l, c), "", Kind::Backspace)
        } else if l > 0 {
            let prev = clen(&self.lines[l - 1]);
            self.replace_with((l - 1, prev), (l, 0), "", Kind::Other)
        } else {
            Ok(())
        }
    }

    pub fn delete(&mut self) -> Result<(), EditError> {
        if self.delete_selection_kind(Kind::Other)? {
            return Ok(());
        }
        let (l, c) = self.cursor;
        if c < clen(&self.lines[l]) {
            self.replace_with((l, c), (l, c + 1), "", Kind::Delete)
        } else if l + 1 < self.lines.len() {
            self.replace_with((l, c), (l + 1, 0), "", Kind::Other)
        } else {
            Ok(())
        }
    }

    /// Tab: indent the selected lines, or insert spaces up to the next tab stop.
    pub fn indent(&mut self) -> Result<(), EditError> {
        let pad = " ".repeat(self.tab_width);
        match self.selection() {
            Some((a, b)) if a.0 != b.0 => {
                let last = if b.1 == 0 { b.0 - 1 } else { b.0 };
                let body: Vec<String> = (a.0..=last).map(|i| if self.lines[i].is_empty() { String::new() } else { format!("{pad}{}", self.lines[i]) }).collect();
                let end_col = clen(&self.lines[last]);
                self.replace_with((a.0, 0), (last, end_col), &body.join("\n"), Kind::Other)?;
                self.select_range((a.0, 0), (last, clen(&self.lines[last])));
                Ok(())
            }
            _ => {
                let c = self.selection().map_or(self.cursor.1, |(a, _)| a.1);
                let n = self.tab_width - c % self.tab_width;
                self.insert(&" ".repeat(n))
            }
        }
    }

    /// Shift+Tab: remove up to one indent level from the current or selected lines.
    pub fn unindent(&mut self) -> Result<(), EditError> {
        let (first, last) = match self.selection() {
            Some((a, b)) => (a.0, if b.1 == 0 && b.0 > a.0 { b.0 - 1 } else { b.0 }),
            None => (self.cursor.0, self.cursor.0),
        };
        let had_sel = self.selection().is_some();
        let body: Vec<String> = (first..=last)
            .map(|i| {
                let l = &self.lines[i];
                let n = l.chars().take(self.tab_width).take_while(|c| *c == ' ').count();
                let n = if n == 0 && l.starts_with('\t') { 1 } else { n };
                l.chars().skip(n).collect()
            })
            .collect();
        if body.iter().zip(&self.lines[first..=last]).all(|(a, b)| a == b) {
            return Ok(());
        }
        let cur = self.cursor;
        let end_col = clen(&self.lines[last]);
        self.replace_with((first, 0), (last, end_col), &body.join("\n"), Kind::Other)?;
        if had_sel {
            self.select_range((first, 0), (last, clen(&self.lines[last])));
        } else {
            self.set_cursor((cur.0, cur.1.saturating_sub(self.tab_width)), false);
        }
        Ok(())
    }

    /// Delete the current line (or all lines touched by the selection).
    pub fn delete_line(&mut self) -> Result<(), EditError> {
        let (first, last) = self.selection().map_or((self.cursor.0, self.cursor.0), |(a, b)| (a.0, if b.1 == 0 && b.0 > a.0 { b.0 - 1 } else { b.0 }));
        let n = self.lines.len();
        if last + 1 < n {
            self.replace_with((first, 0), (last + 1, 0), "", Kind::Other)
        } else if first > 0 {
            let prev = clen(&self.lines[first - 1]);
            self.replace_with((first - 1, prev), (last, clen(&self.lines[last])), "", Kind::Other)
        } else {
            self.replace_with((0, 0), (last, clen(&self.lines[last])), "", Kind::Other)
        }
    }

    pub fn cut(&mut self) -> Result<Option<String>, EditError> {
        let t = self.selected_text();
        if t.is_some() {
            self.delete_selection_kind(Kind::Other)?;
        }
        Ok(t)
    }

    // ------------------------------------------------------------------ brackets, pairs and comments

    fn char_at(&self, p: Pos) -> Option<char> {
        self.lines.get(p.0)?.chars().nth(p.1)
    }

    /// The bracket next to the caret (just before it, else just after it) and the one that closes or opens it, as
    /// positions. Nesting is counted; the search looks at most 20 000 characters away.
    pub fn match_bracket(&self) -> Option<(Pos, Pos)> {
        const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
        let (line, col) = self.cursor;
        let candidates = [col.checked_sub(1).map(|c| (line, c)), Some((line, col))];
        for here in candidates.into_iter().flatten() {
            let Some(c) = self.char_at(here) else { continue };
            for (open, close) in PAIRS {
                if c == open {
                    let (mut depth, mut seen) = (0i32, 0usize);
                    let (mut l, mut k) = here;
                    while l < self.lines.len() {
                        let chars: Vec<char> = self.lines[l].chars().collect();
                        while k < chars.len() {
                            seen += 1;
                            if seen > 20_000 {
                                return None;
                            }
                            if chars[k] == open {
                                depth += 1;
                            } else if chars[k] == close {
                                depth -= 1;
                                if depth == 0 {
                                    return Some((here, (l, k)));
                                }
                            }
                            k += 1;
                        }
                        l += 1;
                        k = 0;
                    }
                    return None;
                } else if c == close {
                    let (mut depth, mut seen) = (0i32, 0usize);
                    let (mut l, mut k) = (here.0 as i64, here.1 as i64);
                    while l >= 0 {
                        let chars: Vec<char> = self.lines[l as usize].chars().collect();
                        if k >= chars.len() as i64 || k < 0 {
                            k = chars.len() as i64 - 1;
                        }
                        while k >= 0 {
                            seen += 1;
                            if seen > 20_000 {
                                return None;
                            }
                            let ch = chars[k as usize];
                            if ch == close {
                                depth += 1;
                            } else if ch == open {
                                depth -= 1;
                                if depth == 0 {
                                    return Some(((l as usize, k as usize), here));
                                }
                            }
                            k -= 1;
                        }
                        l -= 1;
                        k = i64::MAX;
                    }
                    return None;
                }
            }
        }
        None
    }

    /// Type one character the way a code editor does: an opening bracket or quote also writes its partner (or wraps the
    /// selection), typing the partner when it is already next to the caret just steps over it, and a quote after a letter
    /// (an apostrophe) stays single.
    pub fn type_char(&mut self, c: char) -> Result<(), EditError> {
        let partner = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' | '\'' | '`' => Some(c),
            _ => None,
        };
        let closing = matches!(c, ')' | ']' | '}' | '"' | '\'' | '`');
        if let Some(sel) = self.selection() {
            if let Some(p) = partner {
                // wrap what is selected
                let text = self.selected_text().unwrap_or_default();
                self.replace(sel.0, sel.1, &format!("{c}{text}{p}"))?;
                let end = end_of(sel.0, &format!("{c}{text}"));
                self.select_range((sel.0 .0, sel.0 .1 + 1), (end.0, end.1));
                return Ok(());
            }
        } else if closing && self.char_at(self.cursor) == Some(c) {
            self.set_cursor((self.cursor.0, self.cursor.1 + 1), false);
            return Ok(());
        }
        let before = self.cursor.1.checked_sub(1).and_then(|k| self.char_at((self.cursor.0, k)));
        let after = self.char_at(self.cursor);
        let quote = matches!(c, '"' | '\'' | '`');
        let word = |x: Option<char>| x.is_some_and(|x| x.is_alphanumeric() || x == '_');
        let pair = match partner {
            Some(p) if quote => (!word(before) && !word(after) && after != Some(c) && before != Some(c)).then_some(p),
            Some(p) => (after.is_none() || after.is_some_and(|a| a.is_whitespace() || matches!(a, ')' | ']' | '}' | ',' | ';'))).then_some(p),
            None => None,
        };
        match pair {
            Some(p) => {
                self.insert(&format!("{c}{p}"))?;
                self.set_cursor((self.cursor.0, self.cursor.1 - 1), false);
                Ok(())
            }
            None => self.insert(&c.to_string()),
        }
    }

    /// Backspace that removes a pair written by [`Buffer::type_char`] when the caret is between its two halves.
    pub fn backspace_pair(&mut self) -> Result<(), EditError> {
        if self.selection().is_none() && self.cursor.1 > 0 {
            let (l, c) = self.cursor;
            if let (Some(a), Some(b)) = (self.char_at((l, c - 1)), self.char_at((l, c))) {
                if matches!((a, b), ('(', ')') | ('[', ']') | ('{', '}') | ('"', '"') | ('\'', '\'') | ('`', '`')) {
                    return self.replace((l, c - 1), (l, c + 1), "");
                }
            }
        }
        self.backspace()
    }

    /// Comment the selected lines (or the line the caret is on) with `prefix`, or take the comment off when every
    /// non-blank line already has it. The comment goes at the shallowest indent so the lines stay lined up.
    pub fn toggle_comment(&mut self, prefix: &str) -> Result<(), EditError> {
        let (first, last) = match self.selection() {
            Some((a, b)) => (a.0, if b.1 == 0 && b.0 > a.0 { b.0 - 1 } else { b.0 }),
            None => (self.cursor.0, self.cursor.0),
        };
        let had_sel = self.selection().is_some();
        let indent = |l: &str| l.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let body_lines: Vec<usize> = (first..=last).filter(|i| !self.lines[*i].trim().is_empty()).collect();
        if body_lines.is_empty() {
            return Ok(());
        }
        let all_commented = body_lines.iter().all(|i| self.lines[*i].trim_start().starts_with(prefix));
        let min_indent = body_lines.iter().map(|i| indent(&self.lines[*i])).min().unwrap_or(0);
        let cur = self.cursor;
        let old_len = clen(&self.lines[cur.0]);
        let out: Vec<String> = (first..=last)
            .map(|i| {
                let l = &self.lines[i];
                if l.trim().is_empty() {
                    return l.clone();
                }
                let ind = indent(l);
                let chars: Vec<char> = l.chars().collect();
                if all_commented {
                    let rest: String = chars[ind..].iter().collect();
                    let rest = rest.strip_prefix(prefix).unwrap_or(&rest);
                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                    format!("{}{rest}", chars[..ind].iter().collect::<String>())
                } else {
                    format!("{}{prefix} {}", chars[..min_indent].iter().collect::<String>(), chars[min_indent..].iter().collect::<String>())
                }
            })
            .collect();
        let end_col = clen(&self.lines[last]);
        self.replace_with((first, 0), (last, end_col), &out.join("\n"), Kind::Other)?;
        if had_sel {
            self.select_range((first, 0), (last, clen(&self.lines[last])));
        } else {
            let delta = clen(&out[cur.0 - first]) as i64 - old_len as i64;
            self.set_cursor((cur.0, (cur.1 as i64 + delta).max(0) as usize), false);
        }
        Ok(())
    }

    // ------------------------------------------------------------------ undo / redo

    pub fn undo(&mut self) -> bool {
        let Some(e) = self.undo.pop() else { return false };
        let end = end_of(e.start, &e.inserted);
        self.raw_replace(e.start, end, &e.removed);
        self.cursor = e.cursor_before;
        self.anchor = e.anchor_before;
        self.want_col = self.cursor.1;
        self.redo.push(e);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(e) = self.redo.pop() else { return false };
        let end = end_of(e.start, &e.removed);
        let after = self.raw_replace(e.start, end, &e.inserted);
        self.cursor = after;
        self.anchor = None;
        self.want_col = self.cursor.1;
        self.undo.push(e);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    // ------------------------------------------------------------------ whole-document replacement

    /// Replace the whole text in one undo step, touching only the lines that differ (so the cursor
    /// and the highlighter cache survive edits far from them). Used to apply accepted proposals.
    pub fn set_text_undoable(&mut self, new_text: &str) -> Result<(), EditError> {
        let new_text = new_text.replace("\r\n", "\n");
        let body = new_text.strip_suffix('\n').unwrap_or(&new_text);
        let new_lines: Vec<&str> = body.split('\n').collect();
        let old = &self.lines;
        let mut pre = 0;
        while pre < old.len() && pre < new_lines.len() && old[pre] == new_lines[pre] {
            pre += 1;
        }
        let mut suf = 0;
        while suf < old.len() - pre && suf < new_lines.len() - pre && old[old.len() - 1 - suf] == new_lines[new_lines.len() - 1 - suf] {
            suf += 1;
        }
        let (a, b): (Pos, Pos) = if pre == old.len() && pre == new_lines.len() {
            return Ok(()); // identical
        } else if pre == old.len() {
            // pure append
            let l = old.len() - 1;
            ((l, clen(&old[l])), (l, clen(&old[l])))
        } else {
            ((pre, 0), (old.len() - suf - 1, clen(&old[old.len() - suf - 1])))
        };
        let mid: Vec<&str> = new_lines[pre..new_lines.len() - suf].to_vec();
        let text = if pre == old.len() { format!("\n{}", mid.join("\n")) } else { mid.join("\n") };
        // inserting zero lines in the middle: remove whole old lines including one separator
        if mid.is_empty() && pre < old.len() {
            let (a2, b2) = if old.len() - suf < old.len() { ((pre, 0), (old.len() - suf, 0)) } else { (a, b) };
            if old.len() - suf < old.len() {
                return self.replace_with(a2, b2, "", Kind::Other).map(|_| self.finish_set(new_text.ends_with('\n')));
            }
            if pre > 0 {
                return self.replace_with((pre - 1, clen(&self.lines[pre - 1])), b, "", Kind::Other).map(|_| self.finish_set(new_text.ends_with('\n')));
            }
        }
        let trailing = new_text.ends_with('\n');
        self.replace_with(a, b, &text, Kind::Other)?;
        self.finish_set(trailing);
        Ok(())
    }

    fn finish_set(&mut self, _trailing: bool) {
        self.cursor = self.clamp(self.cursor);
        self.anchor = None;
    }

    // ------------------------------------------------------------------ search

    /// All matches (single-line, non-overlapping) as (start, end) positions.
    pub fn find_all(&self, query: &str, case_sensitive: bool) -> Vec<(Pos, Pos)> {
        if query.is_empty() || query.contains('\n') {
            return vec![];
        }
        let q: Vec<char> = fold(query, case_sensitive);
        let mut out = Vec::new();
        for (li, line) in self.lines.iter().enumerate() {
            let chars = fold(line, case_sensitive);
            let mut i = 0;
            while i + q.len() <= chars.len() {
                if chars[i..i + q.len()] == q[..] {
                    out.push(((li, i), (li, i + q.len())));
                    i += q.len();
                } else {
                    i += 1;
                }
            }
        }
        out
    }

    /// The next match after (or before) `from`, wrapping around.
    pub fn find_next(&self, query: &str, case_sensitive: bool, from: Pos, forward: bool) -> Option<(Pos, Pos)> {
        let all = self.find_all(query, case_sensitive);
        if all.is_empty() {
            return None;
        }
        if forward {
            all.iter().find(|(a, _)| *a >= from).or_else(|| all.first()).copied()
        } else {
            all.iter().rev().find(|(_, b)| *b <= from).or_else(|| all.last()).copied()
        }
    }

    /// Select the next match and move the cursor to it.
    pub fn select_next(&mut self, query: &str, case_sensitive: bool, forward: bool) -> bool {
        let from = match (self.selection(), forward) {
            (Some((a, b)), true) => b.max(a),
            (Some((a, _)), false) => a,
            (None, _) => self.cursor,
        };
        match self.find_next(query, case_sensitive, from, forward) {
            Some((a, b)) => {
                self.select_range(a, b);
                true
            }
            None => false,
        }
    }

    /// Replace every match in one undo step; returns how many were replaced.
    pub fn replace_all(&mut self, query: &str, with: &str, case_sensitive: bool) -> Result<usize, EditError> {
        let all = self.find_all(query, case_sensitive);
        if all.is_empty() {
            return Ok(0);
        }
        let with = with.replace('\n', " ");
        let mut lines = self.lines.clone();
        for (a, b) in all.iter().rev() {
            let l = &mut lines[a.0];
            let (x, y) = (byte_of(l, a.1), byte_of(l, b.1));
            l.replace_range(x..y, &with);
        }
        let mut text = lines.join("\n");
        if self.trailing_newline {
            text.push('\n');
        }
        self.set_text_undoable(&text)?;
        Ok(all.len())
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn fold(s: &str, case_sensitive: bool) -> Vec<char> {
    if case_sensitive { s.chars().collect() } else { s.chars().flat_map(|c| c.to_lowercase()).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Buffer {
        Buffer::from_text(s).unwrap()
    }

    #[test]
    fn loads_and_saves_text_faithfully() {
        for t in ["", "one", "one\n", "a\nb\nc", "a\nb\nc\n", "\n", "\n\n", "tab\there\n", "ünï\n"] {
            assert_eq!(b(t).text(), if t.is_empty() { "" } else { t }, "{t:?}");
        }
        let crlf = b("a\r\nb\r\n");
        assert!(crlf.uses_crlf());
        assert_eq!(crlf.line_count(), 2);
        assert_eq!(crlf.text(), "a\r\nb\r\n");
        assert_eq!(b("no newline at end").text(), "no newline at end");
        assert_eq!(Buffer::from_text("a\0b").unwrap_err(), EditError::Binary);
        assert_eq!(Buffer::from_text(&"x".repeat(MAX_BYTES + 1)).unwrap_err(), EditError::TooLarge);
        assert_eq!(b("abc\ndef").byte_len(), 7);
    }

    #[test]
    fn typing_moves_the_cursor_and_unicode_columns_count_chars() {
        let mut x = b("");
        for c in "héllo".chars() {
            x.insert(&c.to_string()).unwrap();
        }
        assert_eq!((x.line(0), x.cursor), ("héllo", (0, 5)));
        x.move_to(Move::Left, false);
        x.insert("!").unwrap();
        assert_eq!(x.line(0), "héll!o");
        x.backspace().unwrap();
        x.backspace().unwrap();
        assert_eq!(x.line(0), "hélo");
        x.delete().unwrap();
        assert_eq!(x.line(0), "hél");
    }

    #[test]
    fn enter_keeps_indentation_and_deepens_after_openers() {
        let mut x = b("    let a = 1;");
        x.move_to(Move::End, false);
        x.newline().unwrap();
        assert_eq!((x.line(1), x.cursor), ("    ", (1, 4)));
        let mut y = b("fn main() {");
        y.move_to(Move::End, false);
        y.newline().unwrap();
        assert_eq!(y.line(1), "    ");
        let mut z = b("if x:");
        z.move_to(Move::End, false);
        z.insert("\n").unwrap();
        assert_eq!(z.line(1), "    ");
        // splitting a line in the middle
        let mut w = b("hello world");
        w.set_cursor((0, 5), false);
        w.newline().unwrap();
        assert_eq!((w.line(0), w.line(1)), ("hello", " world"));
    }

    #[test]
    fn backspace_and_delete_join_lines_and_unindent() {
        let mut x = b("ab\ncd");
        x.set_cursor((1, 0), false);
        x.backspace().unwrap();
        assert_eq!((x.line_count(), x.line(0), x.cursor), (1, "abcd", (0, 2)));
        x.set_cursor((0, 4), false);
        x.delete().unwrap();
        assert_eq!(x.line(0), "abcd", "delete at the very end does nothing");
        let mut y = b("a\nb");
        y.set_cursor((0, 1), false);
        y.delete().unwrap();
        assert_eq!(y.line(0), "ab");
        let mut z = b("        x");
        z.set_cursor((0, 8), false);
        z.backspace().unwrap();
        assert_eq!(z.line(0), "    x", "backspace in indentation removes a whole tab stop");
        z.set_cursor((0, 0), false);
        z.backspace().unwrap();
        assert_eq!(z.line(0), "    x");
    }

    #[test]
    fn selection_copy_cut_replace_and_select_all() {
        let mut x = b("hello world\nsecond line");
        x.set_cursor((0, 6), false);
        x.move_to(Move::End, true);
        assert_eq!(x.selected_text().as_deref(), Some("world"));
        x.move_to(Move::Down, true);
        assert_eq!(x.selected_text().as_deref(), Some("world\nsecond line"));
        let cut = x.cut().unwrap();
        assert_eq!(cut.as_deref(), Some("world\nsecond line"));
        assert_eq!(x.text(), "hello ");
        x.select_all();
        x.insert("X").unwrap();
        assert_eq!(x.text(), "X");
        // arrows collapse a selection
        let mut y = b("abcdef");
        y.select_range((0, 1), (0, 4));
        y.move_to(Move::Left, false);
        assert_eq!((y.cursor, y.selection()), ((0, 1), None));
        y.select_range((0, 1), (0, 4));
        y.move_to(Move::Right, false);
        assert_eq!(y.cursor, (0, 4));
        // typing over a selection replaces it
        y.select_range((0, 0), (0, 3));
        y.insert("Z").unwrap();
        assert_eq!(y.line(0), "Zdef");
    }

    #[test]
    fn vertical_movement_remembers_the_wanted_column() {
        let mut x = b("a long line here\nxy\nanother long one");
        x.set_cursor((0, 10), false);
        x.move_to(Move::Down, false);
        assert_eq!(x.cursor, (1, 2));
        x.move_to(Move::Down, false);
        assert_eq!(x.cursor, (2, 10), "the column springs back");
        x.move_to(Move::Down, false);
        assert_eq!(x.cursor, (2, 16), "past the last line goes to its end");
        x.move_to(Move::DocStart, false);
        x.move_to(Move::Up, false);
        assert_eq!(x.cursor, (0, 0));
        x.move_to(Move::PageDown(2), false);
        assert_eq!(x.cursor.0, 2);
        x.move_to(Move::PageUp(5), false);
        assert_eq!(x.cursor.0, 0);
    }

    #[test]
    fn home_toggles_between_first_character_and_column_zero_and_words_jump() {
        let mut x = b("    indented text");
        x.set_cursor((0, 10), false);
        x.move_to(Move::Home, false);
        assert_eq!(x.cursor, (0, 4));
        x.move_to(Move::Home, false);
        assert_eq!(x.cursor, (0, 0));
        x.move_to(Move::End, false);
        assert_eq!(x.cursor, (0, 17));
        x.move_to(Move::WordLeft, false);
        assert_eq!(x.cursor, (0, 13));
        x.move_to(Move::WordLeft, false);
        assert_eq!(x.cursor, (0, 4));
        x.move_to(Move::WordRight, false);
        assert_eq!(x.cursor, (0, 12));
        let mut y = b("foo\n\nbar");
        y.move_to(Move::WordRight, false);
        assert_eq!(y.cursor, (0, 3));
        y.move_to(Move::WordRight, false);
        assert_eq!(y.cursor, (2, 3));
        y.move_to(Move::WordLeft, false);
        assert_eq!(y.cursor, (2, 0));
        y.move_to(Move::WordLeft, false);
        assert_eq!(y.cursor, (1, 0));
    }

    #[test]
    fn undo_and_redo_restore_text_and_cursor_and_merge_typing() {
        let mut x = b("start");
        x.move_to(Move::End, false);
        for c in " more".chars() {
            x.insert(&c.to_string()).unwrap();
        }
        assert_eq!(x.line(0), "start more");
        assert!(x.undo());
        assert_eq!((x.line(0), x.cursor), ("start", (0, 5)), "one undo removes the whole typed word run");
        assert!(!x.undo());
        assert!(x.redo());
        assert_eq!((x.line(0), x.cursor), ("start more", (0, 10)));
        // backspace runs merge too
        x.backspace().unwrap();
        x.backspace().unwrap();
        x.backspace().unwrap();
        assert_eq!(x.line(0), "start m");
        x.undo();
        assert_eq!(x.line(0), "start more");
        // a new edit clears redo
        x.insert("!").unwrap();
        assert!(!x.can_redo());
        // multi-line edits
        let mut y = b("a\nb\nc");
        y.select_all();
        y.insert("x\ny").unwrap();
        assert_eq!(y.text(), "x\ny");
        y.undo();
        assert_eq!(y.text(), "a\nb\nc");
        assert_eq!(y.selection(), Some(((0, 0), (2, 1))), "the selection comes back");
        y.redo();
        assert_eq!(y.text(), "x\ny");
    }

    #[test]
    fn dirty_tracking_and_revision() {
        let mut x = b("a");
        assert!(!x.is_dirty());
        x.insert("b").unwrap();
        assert!(x.is_dirty());
        x.mark_saved();
        assert!(!x.is_dirty());
        x.undo();
        assert!(x.is_dirty(), "undo past the saved point is a change");
        x.redo();
        assert!(!x.is_dirty(), "and redo back to it is clean again");
        assert_eq!(x.take_dirty(), Some(0));
        assert_eq!(x.take_dirty(), None);
    }

    #[test]
    fn indent_and_unindent_work_on_lines_and_selections() {
        let mut x = b("a\nb\n\nc");
        x.indent().unwrap();
        assert_eq!(x.line(0), "    a", "no selection: spaces to the next tab stop");
        let mut y = b("a\nb\n\nc");
        y.select_range((0, 0), (3, 1));
        y.indent().unwrap();
        assert_eq!(y.text(), "    a\n    b\n\n    c", "blank lines stay blank");
        y.unindent().unwrap();
        assert_eq!(y.text(), "a\nb\n\nc");
        y.unindent().unwrap();
        assert_eq!(y.text(), "a\nb\n\nc", "nothing left to remove");
        let mut z = b("\tx");
        z.unindent().unwrap();
        assert_eq!(z.line(0), "x");
        let mut w = b("ab");
        w.set_cursor((0, 1), false);
        w.indent().unwrap();
        assert_eq!(w.line(0), "a   b", "to the next stop, not a fixed four");
        // undo reverts a whole block indent in one step
        let mut v = b("a\nb");
        v.select_all();
        v.indent().unwrap();
        v.undo();
        assert_eq!(v.text(), "a\nb");
    }

    #[test]
    fn delete_line_handles_first_middle_and_last() {
        let mut x = b("a\nb\nc");
        x.set_cursor((1, 0), false);
        x.delete_line().unwrap();
        assert_eq!(x.text(), "a\nc");
        x.set_cursor((1, 0), false);
        x.delete_line().unwrap();
        assert_eq!(x.text(), "a");
        x.delete_line().unwrap();
        assert_eq!(x.text(), "");
        assert_eq!(x.line_count(), 1);
    }

    #[test]
    fn find_next_prev_wraps_and_respects_case() {
        let x = b("Foo foo\nbar FOO");
        assert_eq!(x.find_all("foo", false).len(), 3);
        assert_eq!(x.find_all("foo", true), vec![((0, 4), (0, 7))]);
        assert_eq!(x.find_next("foo", false, (0, 1), true), Some(((0, 4), (0, 7))));
        assert_eq!(x.find_next("foo", false, (1, 5), true), Some(((0, 0), (0, 3))), "wraps to the top");
        assert_eq!(x.find_next("foo", false, (0, 0), false), Some(((1, 4), (1, 7))), "backwards wraps to the end");
        assert_eq!(x.find_next("zzz", false, (0, 0), true), None);
        assert!(x.find_all("", false).is_empty());
        let mut y = b("aaa");
        assert_eq!(y.find_all("aa", true).len(), 1, "matches do not overlap");
        assert!(y.select_next("a", true, true));
        assert_eq!(y.selection(), Some(((0, 0), (0, 1))));
        assert!(y.select_next("a", true, true));
        assert_eq!(y.selection(), Some(((0, 1), (0, 2))));
        assert!(y.select_next("a", true, false));
        assert_eq!(y.selection(), Some(((0, 0), (0, 1))));
    }

    #[test]
    fn replace_all_is_one_undo_step() {
        let mut x = b("one two one\nthree one\n");
        assert_eq!(x.replace_all("one", "1", true).unwrap(), 3);
        assert_eq!(x.text(), "1 two 1\nthree 1\n");
        x.undo();
        assert_eq!(x.text(), "one two one\nthree one\n");
        assert_eq!(x.replace_all("zzz", "y", true).unwrap(), 0);
        assert!(!x.is_dirty(), "undoing back to the loaded state is clean");
    }

    #[test]
    fn set_text_undoable_touches_only_what_changed() {
        let mut x = b("a\nb\nc\nd\n");
        x.set_cursor((0, 1), false);
        x.set_text_undoable("a\nB\nc\nd\n").unwrap();
        assert_eq!(x.text(), "a\nB\nc\nd\n");
        assert_eq!(x.take_dirty(), Some(1), "only line 1 was invalidated");
        assert_eq!(x.cursor.0, 1);
        x.undo();
        assert_eq!(x.text(), "a\nb\nc\nd\n");
        for (from, to) in [("a\nb\n", "a\nb\nc\n"), ("a\nb\nc\n", "a\nc\n"), ("a\n", "b\n"), ("a\nb\n", ""), ("", "x\ny\n"), ("a\nb\nc\n", "a\nb\nc\nd\ne\n"), ("a\nb\nc\n", "c\n"), ("a\nb\nc\n", "a\n"), ("x\n", "x\n")] {
            let mut y = b(from);
            y.set_text_undoable(to).unwrap();
            assert_eq!(y.text(), to, "{from:?} -> {to:?}");
            y.undo();
            assert_eq!(y.text(), from, "undo {from:?} -> {to:?}");
            y.redo();
            assert_eq!(y.text(), to, "redo {from:?} -> {to:?}");
        }
    }

    #[test]
    fn size_limit_is_enforced_on_edits() {
        let mut x = b(&"y".repeat(MAX_BYTES - 5));
        assert!(x.insert("1234").is_ok());
        assert_eq!(x.insert("123456789").unwrap_err(), EditError::TooLarge);
        assert_eq!(x.byte_len(), MAX_BYTES - 1);
    }

    #[test]
    fn crlf_pasted_text_is_normalised_and_line_endings_survive() {
        let mut x = b("a\r\nb\r\n");
        x.set_cursor((0, 1), false);
        x.insert("1\r\n2").unwrap();
        assert_eq!(x.text(), "a1\r\n2\r\nb\r\n");
        assert_eq!(x.line_count(), 3);
    }

    #[test]
    fn random_edits_never_break_invariants_and_undo_everything() {
        // a tiny deterministic generator
        let mut s = 0x1234_5678_9abc_def0u64;
        let mut rnd = move |n: usize| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % n as u64) as usize
        };
        let mut x = b("fn main() {\n    println!(\"héllo\");\n}\n");
        let original = x.text();
        let mut steps = 0;
        for _ in 0..600 {
            match rnd(12) {
                0 => x.insert("a").unwrap(),
                1 => x.insert("é").unwrap(),
                2 => x.insert("\n").unwrap(),
                3 => x.insert("multi\nline\npaste").unwrap(),
                4 => x.backspace().unwrap(),
                5 => x.delete().unwrap(),
                6 => x.move_to([Move::Left, Move::Right, Move::Up, Move::Down, Move::Home, Move::End][rnd(6)], rnd(2) == 0),
                7 => x.indent().unwrap(),
                8 => x.unindent().unwrap(),
                9 => x.select_all(),
                10 => {
                    let _ = x.cut();
                }
                _ => x.delete_line().unwrap(),
            }
            steps += 1;
            assert!(x.cursor.0 < x.line_count() && x.cursor.1 <= clen(x.line(x.cursor.0)), "cursor in range after step {steps}");
            assert_eq!(x.byte_len(), x.lines().iter().map(|l| l.len()).sum::<usize>() + x.line_count() - 1, "byte count in sync");
            assert!(!x.lines().iter().any(|l| l.contains('\n')), "no embedded newlines");
        }
        while x.undo() {}
        assert_eq!(x.text(), original, "undoing everything returns to the start");
    }

    // ------------------------------------------------------------------ brackets, pairs and comments

    fn at(text: &str, pos: Pos) -> Buffer {
        let mut b = Buffer::from_text(text).unwrap();
        b.set_cursor(pos, false);
        b
    }

    #[test]
    fn the_bracket_next_to_the_caret_is_matched_across_lines_and_nesting() {
        let b = at("fn f(a: (u8, u8)) {\n    g([1, 2]);\n}", (0, 5));
        assert_eq!(b.match_bracket(), Some(((0, 4), (0, 16))), "caret just after the opening bracket: the outer pair");
        let b = at("fn f(a: (u8, u8)) {\n    g([1, 2]);\n}", (0, 17));
        assert_eq!(b.match_bracket(), Some(((0, 4), (0, 16))), "caret just after the closing one");
        let b = at("fn f() {\n    g([1, 2]);\n}", (0, 7));
        assert_eq!(b.match_bracket(), Some(((0, 7), (2, 0))), "braces match over lines (caret before the opening one)");
        let b = at("fn f() {\n    g([1, 2]);\n}", (2, 1));
        assert_eq!(b.match_bracket(), Some(((0, 7), (2, 0))), "and from the closing one back");
        assert_eq!(at("(a", (0, 1)).match_bracket(), None, "an unmatched bracket has no partner");
        assert_eq!(at("no brackets", (0, 3)).match_bracket(), None);
        let long = format!("({})", "x".repeat(25_000));
        assert_eq!(at(&long, (0, 1)).match_bracket(), None, "very far partners are not searched for");
    }

    #[test]
    fn opening_brackets_and_quotes_write_their_partner_and_closing_ones_step_over() {
        let mut b = at("", (0, 0));
        b.type_char('(').unwrap();
        assert_eq!((b.line(0), b.cursor), ("()", (0, 1)));
        b.type_char('a').unwrap();
        b.type_char(')').unwrap();
        assert_eq!((b.line(0), b.cursor), ("(a)", (0, 3)), "typing the closer steps over it");
        let mut b = at("", (0, 0));
        for c in "f(\"x\")".chars() {
            b.type_char(c).unwrap();
        }
        assert_eq!(b.line(0), "f(\"x\")");
        // an apostrophe after a letter stays single, a quote before a letter does not pair
        let mut b = at("don", (0, 3));
        b.type_char('\'').unwrap();
        assert_eq!(b.line(0), "don'");
        let mut b = at("word", (0, 0));
        b.type_char('(').unwrap();
        assert_eq!(b.line(0), "(word", "no partner before a word");
        // wrapping a selection
        let mut b = at("hello world", (0, 0));
        b.select_range((0, 0), (0, 5));
        b.type_char('"').unwrap();
        assert_eq!(b.line(0), "\"hello\" world");
        assert_eq!(b.selected_text().as_deref(), Some("hello"), "the wrapped text stays selected");
        // backspace between a pair removes both, elsewhere one
        let mut b = at("()", (0, 1));
        b.backspace_pair().unwrap();
        assert_eq!(b.line(0), "");
        let mut b = at("(x)", (0, 2));
        b.backspace_pair().unwrap();
        assert_eq!(b.line(0), "()");
        // one undo takes the typed pair back
        let mut b = at("", (0, 0));
        b.type_char('{').unwrap();
        assert!(b.undo());
        assert_eq!(b.line(0), "");
    }

    #[test]
    fn comments_toggle_on_lines_keep_the_indent_and_come_off_again() {
        let mut b = at("fn a() {\n    let x = 1;\n\n    let y = 2;\n}", (1, 6));
        b.select_range((1, 0), (3, 14));
        b.toggle_comment("//").unwrap();
        assert_eq!(b.text(), "fn a() {\n    // let x = 1;\n\n    // let y = 2;\n}", "the blank line is left alone");
        b.toggle_comment("//").unwrap();
        assert_eq!(b.text(), "fn a() {\n    let x = 1;\n\n    let y = 2;\n}", "all commented: the comment comes off");
        // a mixture gets commented as a whole at the shallowest indent
        let mut b = at("    a\n  // b\n      c", (0, 0));
        b.select_range((0, 0), (2, 7));
        b.toggle_comment("//").unwrap();
        assert_eq!(b.text(), "  //   a\n  // // b\n  //     c");
        // one line, with the caret moving along with the text
        let mut b = at("x = 1", (0, 4));
        b.toggle_comment("#").unwrap();
        assert_eq!((b.text().as_str(), b.cursor), ("# x = 1", (0, 6)));
        b.toggle_comment("#").unwrap();
        assert_eq!((b.text().as_str(), b.cursor), ("x = 1", (0, 4)));
        // undo is one step
        assert!(b.undo());
        assert_eq!(b.text(), "# x = 1");
        // an empty line has nothing to comment
        let mut b = at("", (0, 0));
        b.toggle_comment("//").unwrap();
        assert_eq!(b.line(0), "");
    }
}
