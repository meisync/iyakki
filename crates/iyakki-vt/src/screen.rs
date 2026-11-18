//! The screen of a terminal: a grid of cells with colours and attributes, a cursor, scroll region, tab stops, the alternate screen
//! and a scrollback. [`crate::Parser`] drives it from the bytes a program writes; nothing here knows about processes or the display.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    /// 0..=7 normal, 8..=15 bright, 16..=255 the xterm 6x6x6 cube and grey ramp.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

pub const BOLD: u8 = 1;
pub const DIM: u8 = 2;
pub const ITALIC: u8 = 4;
pub const UNDERLINE: u8 = 8;
pub const BLINK: u8 = 16;
pub const REVERSE: u8 = 32;
pub const HIDDEN: u8 = 64;
pub const STRIKE: u8 = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: u8,
    /// The right half of a double-width character: nothing is drawn here.
    pub wide_tail: bool,
}

impl Default for Cell {
    fn default() -> Cell {
        Cell { ch: ' ', fg: Color::Default, bg: Color::Default, attrs: 0, wide_tail: false }
    }
}

/// The pen: what new characters are drawn with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pen {
    pub fg: Color,
    pub bg: Color,
    pub attrs: u8,
}

#[derive(Clone, Debug, Default)]
struct Saved {
    x: usize,
    y: usize,
    pen: Pen,
    origin: bool,
    pending_wrap: bool,
    line_drawing: bool,
}

pub const MAX_COLS: usize = 500;
pub const MAX_ROWS: usize = 200;
pub const DEFAULT_SCROLLBACK: usize = 2000;

/// How many columns a character takes: 0 for combining marks and format characters, 2 for East Asian wide characters, else 1.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    if u < 0x20 || (0x7f..0xa0).contains(&u) {
        return 0;
    }
    if (0x300..=0x36f).contains(&u) || (0x200b..=0x200f).contains(&u) || (0xfe00..=0xfe0f).contains(&u) || u == 0xfeff || (0x20d0..=0x20ff).contains(&u) {
        return 0;
    }
    let wide = (0x1100..=0x115f).contains(&u)
        || (0x2e80..=0x303e).contains(&u)
        || (0x3041..=0x33ff).contains(&u)
        || (0x3400..=0x4dbf).contains(&u)
        || (0x4e00..=0x9fff).contains(&u)
        || (0xa000..=0xa4cf).contains(&u)
        || (0xac00..=0xd7a3).contains(&u)
        || (0xf900..=0xfaff).contains(&u)
        || (0xfe30..=0xfe6f).contains(&u)
        || (0xff00..=0xff60).contains(&u)
        || (0xffe0..=0xffe6).contains(&u)
        || (0x1f300..=0x1f64f).contains(&u)
        || (0x1f900..=0x1f9ff).contains(&u)
        || (0x20000..=0x3fffd).contains(&u);
    if wide {
        2
    } else {
        1
    }
}

/// The DEC special graphics set (`ESC ( 0`): line drawing.
pub fn line_drawing(c: char) -> char {
    match c {
        '`' => '◆',
        'a' => '▒',
        'f' => '°',
        'g' => '±',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        _ => c,
    }
}

pub struct Screen {
    cols: usize,
    rows: usize,
    grid: Vec<Vec<Cell>>,
    pub x: usize,
    pub y: usize,
    pub pen: Pen,
    saved: Saved,
    top: usize,
    bottom: usize,
    tabs: Vec<bool>,
    scrollback: VecDeque<Vec<Cell>>,
    scrollback_max: usize,
    alt_saved: Option<(Vec<Vec<Cell>>, usize, usize)>,
    pending_wrap: bool,
    pub title: String,
    // modes
    pub autowrap: bool,
    pub origin: bool,
    pub insert: bool,
    pub newline_mode: bool,
    pub cursor_visible: bool,
    pub app_cursor_keys: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub alt_screen: bool,
    /// Switched on by `ESC ( 0`: the graphics set is G0.
    pub(crate) line_draw: bool,
    /// Bytes to send back to the program (answers to status and attribute requests).
    pub(crate) replies: Vec<u8>,
    /// Counts every change, so a drawing layer can skip a frame when nothing happened.
    pub generation: u64,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Screen {
        let (cols, rows) = (cols.clamp(1, MAX_COLS), rows.clamp(1, MAX_ROWS));
        let mut s = Screen {
            cols,
            rows,
            grid: vec![vec![Cell::default(); cols]; rows],
            x: 0,
            y: 0,
            pen: Pen::default(),
            saved: Saved::default(),
            top: 0,
            bottom: rows - 1,
            tabs: Vec::new(),
            scrollback: VecDeque::new(),
            scrollback_max: DEFAULT_SCROLLBACK,
            alt_saved: None,
            pending_wrap: false,
            title: String::new(),
            autowrap: true,
            origin: false,
            insert: false,
            newline_mode: false,
            cursor_visible: true,
            app_cursor_keys: false,
            app_keypad: false,
            bracketed_paste: false,
            alt_screen: false,
            line_draw: false,
            replies: Vec::new(),
            generation: 0,
        };
        s.reset_tabs();
        s
    }

    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn rows(&self) -> usize {
        self.rows
    }
    pub fn cell(&self, x: usize, y: usize) -> Cell {
        self.grid.get(y).and_then(|r| r.get(x)).copied().unwrap_or_default()
    }
    pub fn set_scrollback_limit(&mut self, lines: usize) {
        self.scrollback_max = lines.min(100_000);
        while self.scrollback.len() > self.scrollback_max {
            self.scrollback.pop_front();
        }
    }
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// The text of screen row `y`, trailing blanks removed.
    pub fn row_text(&self, y: usize) -> String {
        row_to_string(self.grid.get(y).map(|r| r.as_slice()).unwrap_or(&[]))
    }

    /// The whole visible screen as text, one line per row, trailing blank lines kept.
    pub fn text(&self) -> String {
        (0..self.rows).map(|y| self.row_text(y)).collect::<Vec<_>>().join("\n")
    }

    /// The text of a scrollback line, 0 being the oldest.
    pub fn scrollback_text(&self, i: usize) -> Option<String> {
        self.scrollback.get(i).map(|r| row_to_string(r))
    }

    /// The cells of a row of the view that is `back` lines up in the scrollback (0 = the live screen).
    pub fn view_row(&self, back: usize, y: usize) -> Vec<Cell> {
        let back = back.min(self.scrollback.len());
        if back == 0 {
            return self.grid.get(y).cloned().unwrap_or_default();
        }
        // line index counted from the oldest scrollback line
        let idx = self.scrollback.len() - back + y;
        if idx < self.scrollback.len() {
            self.scrollback[idx].clone()
        } else {
            self.grid.get(idx - self.scrollback.len()).cloned().unwrap_or_default()
        }
    }

    fn touch(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn reset_tabs(&mut self) {
        self.tabs = (0..self.cols).map(|i| i % 8 == 0 && i > 0).collect();
    }

    fn blank(&self) -> Cell {
        Cell { ch: ' ', fg: Color::Default, bg: self.pen.bg, attrs: 0, wide_tail: false }
    }

    /// Change the size. Rows that no longer fit at the top go to the scrollback; the cursor stays on the screen.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.clamp(1, MAX_COLS), rows.clamp(1, MAX_ROWS));
        if cols == self.cols && rows == self.rows {
            return;
        }
        for r in self.grid.iter_mut() {
            r.resize(cols, Cell::default());
        }
        if let Some((main, _, _)) = self.alt_saved.as_mut() {
            main.iter_mut().for_each(|r| r.resize(cols, Cell::default()));
            main.resize(rows, vec![Cell::default(); cols]);
        }
        while self.grid.len() > rows {
            // drop blank rows below the cursor first, else push the top row into the scrollback
            if self.grid.len() > self.y + 1 {
                self.grid.pop();
            } else {
                let top = self.grid.remove(0);
                if !self.alt_screen {
                    self.push_scrollback(top);
                }
                self.y = self.y.saturating_sub(1);
            }
        }
        while self.grid.len() < rows {
            self.grid.push(vec![Cell::default(); cols]);
        }
        self.cols = cols;
        self.rows = rows;
        self.top = 0;
        self.bottom = rows - 1;
        self.x = self.x.min(cols - 1);
        self.y = self.y.min(rows - 1);
        self.pending_wrap = false;
        self.reset_tabs();
        self.touch();
    }

    fn push_scrollback(&mut self, row: Vec<Cell>) {
        if self.scrollback_max == 0 {
            return;
        }
        if self.scrollback.len() >= self.scrollback_max {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(row);
    }

    // ----------------------------------------------------------------- drawing

    pub(crate) fn print(&mut self, c: char) {
        let c = if self.line_draw { line_drawing(c) } else { c };
        let w = char_width(c);
        if w == 0 || (w == 2 && self.cols < 2) {
            return; // combining marks and format characters have no cell of their own
        }
        if self.pending_wrap {
            if self.autowrap {
                self.x = 0;
                self.linefeed();
            }
            self.pending_wrap = false;
        }
        if w == 2 && self.x + 1 >= self.cols {
            // a wide character does not fit on this line
            if self.autowrap {
                let b = self.blank();
                let (x, y) = (self.x, self.y);
                self.grid[y][x] = b;
                self.x = 0;
                self.linefeed();
            } else {
                return;
            }
        }
        if self.insert {
            let (y, x) = (self.y, self.x);
            let b = self.blank();
            let row = &mut self.grid[y];
            for _ in 0..w {
                row.pop();
                row.insert(x, b);
            }
        }
        let cell = Cell { ch: c, fg: self.pen.fg, bg: self.pen.bg, attrs: self.pen.attrs, wide_tail: false };
        let (x, y) = (self.x, self.y);
        self.grid[y][x] = cell;
        if w == 2 {
            self.grid[y][x + 1] = Cell { ch: ' ', wide_tail: true, ..cell };
        }
        if x + w >= self.cols {
            self.x = self.cols - 1;
            self.pending_wrap = true;
        } else {
            self.x = x + w;
        }
        self.touch();
    }

    pub(crate) fn linefeed(&mut self) {
        self.pending_wrap = false;
        if self.y == self.bottom {
            self.scroll_up(1);
        } else if self.y + 1 < self.rows {
            self.y += 1;
        }
        self.touch();
    }

    pub(crate) fn reverse_index(&mut self) {
        self.pending_wrap = false;
        if self.y == self.top {
            self.scroll_down(1);
        } else if self.y > 0 {
            self.y -= 1;
        }
        self.touch();
    }

    pub(crate) fn carriage_return(&mut self) {
        self.x = 0;
        self.pending_wrap = false;
    }

    pub(crate) fn backspace(&mut self) {
        self.x = self.x.saturating_sub(1);
        self.pending_wrap = false;
    }

    pub(crate) fn tab(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.x + 1;
            while x < self.cols - 1 && !self.tabs[x] {
                x += 1;
            }
            self.x = x.min(self.cols - 1);
        }
        self.pending_wrap = false;
    }

    pub(crate) fn back_tab(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.x.saturating_sub(1);
            while x > 0 && !self.tabs[x] {
                x -= 1;
            }
            self.x = x;
        }
        self.pending_wrap = false;
    }

    pub(crate) fn set_tab(&mut self) {
        if self.x < self.tabs.len() {
            self.tabs[self.x] = true;
        }
    }

    pub(crate) fn clear_tab(&mut self, all: bool) {
        if all {
            self.tabs.iter_mut().for_each(|t| *t = false);
        } else if self.x < self.tabs.len() {
            self.tabs[self.x] = false;
        }
    }

    pub fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        for _ in 0..n {
            let row = self.grid.remove(self.top);
            if self.top == 0 && self.bottom == self.rows - 1 && !self.alt_screen {
                self.push_scrollback(row);
            }
            let b = self.blank();
            self.grid.insert(self.bottom, vec![b; self.cols]);
        }
        self.touch();
    }

    pub fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        for _ in 0..n {
            self.grid.remove(self.bottom);
            let b = self.blank();
            self.grid.insert(self.top, vec![b; self.cols]);
        }
        self.touch();
    }

    /// Absolute cursor move (rows and columns from 0); with origin mode the row counts from the top of the scroll region.
    pub(crate) fn move_to(&mut self, row: usize, col: usize) {
        let (lo, hi) = if self.origin { (self.top, self.bottom) } else { (0, self.rows - 1) };
        self.y = (lo + row).min(hi);
        self.x = col.min(self.cols - 1);
        self.pending_wrap = false;
    }

    pub(crate) fn move_by(&mut self, dy: isize, dx: isize) {
        // moving up or down stops at the scroll region's edge when the cursor starts inside it
        let lo = if dy < 0 && self.y >= self.top { self.top } else { 0 };
        let hi = if dy > 0 && self.y <= self.bottom { self.bottom } else { self.rows - 1 };
        self.y = (self.y as isize + dy).clamp(lo as isize, hi as isize) as usize;
        self.x = (self.x as isize + dx).clamp(0, self.cols as isize - 1) as usize;
        self.pending_wrap = false;
    }

    pub(crate) fn set_x(&mut self, col: usize) {
        self.x = col.min(self.cols - 1);
        self.pending_wrap = false;
    }

    pub(crate) fn set_y(&mut self, row: usize) {
        let (lo, hi) = if self.origin { (self.top, self.bottom) } else { (0, self.rows - 1) };
        self.y = (lo + row).min(hi);
        self.pending_wrap = false;
    }

    pub(crate) fn set_region(&mut self, top: usize, bottom: usize) {
        if top < bottom && bottom < self.rows {
            self.top = top;
            self.bottom = bottom;
            self.move_to(0, 0);
        } else if top == 0 && bottom == usize::MAX {
            self.top = 0;
            self.bottom = self.rows - 1;
            self.move_to(0, 0);
        }
    }

    pub fn region(&self) -> (usize, usize) {
        (self.top, self.bottom)
    }

    /// 0 cursor to end, 1 start to cursor, 2 all (3 also forgets the scrollback).
    pub(crate) fn erase_display(&mut self, mode: usize) {
        let b = self.blank();
        match mode {
            0 => {
                self.erase_line_part(self.x, self.cols);
                for y in self.y + 1..self.rows {
                    self.grid[y].iter_mut().for_each(|c| *c = b);
                }
            }
            1 => {
                for y in 0..self.y {
                    self.grid[y].iter_mut().for_each(|c| *c = b);
                }
                self.erase_line_part(0, self.x + 1);
            }
            2 | 3 => {
                for r in self.grid.iter_mut() {
                    r.iter_mut().for_each(|c| *c = b);
                }
                if mode == 3 {
                    self.scrollback.clear();
                }
            }
            _ => {}
        }
        self.touch();
    }

    fn erase_line_part(&mut self, from: usize, to: usize) {
        let b = self.blank();
        let to = to.min(self.cols);
        let y = self.y;
        for c in self.grid[y].iter_mut().take(to).skip(from) {
            *c = b;
        }
    }

    pub(crate) fn erase_line(&mut self, mode: usize) {
        match mode {
            0 => self.erase_line_part(self.x, self.cols),
            1 => self.erase_line_part(0, self.x + 1),
            2 => self.erase_line_part(0, self.cols),
            _ => {}
        }
        self.touch();
    }

    pub(crate) fn erase_chars(&mut self, n: usize) {
        self.erase_line_part(self.x, self.x + n.max(1));
        self.touch();
    }

    pub(crate) fn insert_chars(&mut self, n: usize) {
        let (x, y) = (self.x, self.y);
        let b = self.blank();
        let n = n.clamp(1, self.cols - x);
        let row = &mut self.grid[y];
        for _ in 0..n {
            row.pop();
            row.insert(x, b);
        }
        self.touch();
    }

    pub(crate) fn delete_chars(&mut self, n: usize) {
        let (x, y) = (self.x, self.y);
        let b = self.blank();
        let n = n.clamp(1, self.cols - x);
        let row = &mut self.grid[y];
        for _ in 0..n {
            row.remove(x);
            row.push(b);
        }
        self.touch();
    }

    pub(crate) fn insert_lines(&mut self, n: usize) {
        if self.y < self.top || self.y > self.bottom {
            return;
        }
        let n = n.clamp(1, self.bottom - self.y + 1);
        let b = self.blank();
        for _ in 0..n {
            self.grid.remove(self.bottom);
            self.grid.insert(self.y, vec![b; self.cols]);
        }
        self.x = 0;
        self.pending_wrap = false;
        self.touch();
    }

    pub(crate) fn delete_lines(&mut self, n: usize) {
        if self.y < self.top || self.y > self.bottom {
            return;
        }
        let n = n.clamp(1, self.bottom - self.y + 1);
        let b = self.blank();
        for _ in 0..n {
            self.grid.remove(self.y);
            self.grid.insert(self.bottom, vec![b; self.cols]);
        }
        self.x = 0;
        self.pending_wrap = false;
        self.touch();
    }

    pub(crate) fn save_cursor(&mut self) {
        self.saved = Saved { x: self.x, y: self.y, pen: self.pen, origin: self.origin, pending_wrap: self.pending_wrap, line_drawing: self.line_draw };
    }

    pub(crate) fn restore_cursor(&mut self) {
        let s = self.saved.clone();
        self.x = s.x.min(self.cols - 1);
        self.y = s.y.min(self.rows - 1);
        self.pen = s.pen;
        self.origin = s.origin;
        self.pending_wrap = s.pending_wrap;
        self.line_draw = s.line_drawing;
    }

    /// The alternate screen (full-screen programs): the main screen is kept and comes back untouched.
    pub(crate) fn enter_alt(&mut self, save_cursor: bool) {
        if self.alt_screen {
            return;
        }
        if save_cursor {
            self.save_cursor();
        }
        let blank = vec![vec![Cell::default(); self.cols]; self.rows];
        let main = std::mem::replace(&mut self.grid, blank);
        self.alt_saved = Some((main, self.x, self.y));
        self.alt_screen = true;
        self.touch();
    }

    pub(crate) fn leave_alt(&mut self, restore_cursor: bool) {
        if !self.alt_screen {
            return;
        }
        if let Some((main, x, y)) = self.alt_saved.take() {
            self.grid = main;
            self.x = x.min(self.cols - 1);
            self.y = y.min(self.rows - 1);
        }
        self.alt_screen = false;
        if restore_cursor {
            self.restore_cursor();
        }
        self.touch();
    }

    /// Full reset (`ESC c`).
    pub(crate) fn reset(&mut self) {
        let (c, r, max) = (self.cols, self.rows, self.scrollback_max);
        let mut fresh = Screen::new(c, r);
        fresh.scrollback_max = max;
        fresh.scrollback = std::mem::take(&mut self.scrollback);
        fresh.generation = self.generation.wrapping_add(1);
        *self = fresh;
    }

    /// Take the bytes the terminal has to send back to the program (answers to `CSI 6 n`, `CSI c`).
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }
}

fn row_to_string(r: &[Cell]) -> String {
    let s: String = r.iter().filter(|c| !c.wide_tail).map(|c| c.ch).collect();
    s.trim_end().to_string()
}
