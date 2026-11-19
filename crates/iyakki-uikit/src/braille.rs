//! Braille output for the screen reader (F-A11Y-07, first part): the words the reader speaks, as uncontracted English braille (grade 1)
//! cells, cut to the width of a display with panning left and right and routing from a cell back to the character it stands for.
//!
//! Written here from the published dot patterns; no library and no table file. What is NOT here is the driver: the bytes a particular
//! display wants (USB HID braille, a serial protocol) need the device. A driver gives [`Window`] its width and takes the cells.
//!
//! A secret is never translated: [`for_node`] uses the same words the speech uses, which already hide a secret field's text.

use crate::access::Node;

/// The Unicode braille patterns start here; the low eight bits are the dots 1 to 8.
const BASE: u32 = 0x2800;

fn cell(dots: &[u8]) -> char {
    char::from_u32(BASE + dots.iter().fold(0u32, |a, d| a | (1 << (d - 1)))).unwrap_or(' ')
}

/// The dots of a lower case letter a to z.
fn letter_dots(c: char) -> Option<&'static [u8]> {
    Some(match c {
        'a' => &[1],
        'b' => &[1, 2],
        'c' => &[1, 4],
        'd' => &[1, 4, 5],
        'e' => &[1, 5],
        'f' => &[1, 2, 4],
        'g' => &[1, 2, 4, 5],
        'h' => &[1, 2, 5],
        'i' => &[2, 4],
        'j' => &[2, 4, 5],
        'k' => &[1, 3],
        'l' => &[1, 2, 3],
        'm' => &[1, 3, 4],
        'n' => &[1, 3, 4, 5],
        'o' => &[1, 3, 5],
        'p' => &[1, 2, 3, 4],
        'q' => &[1, 2, 3, 4, 5],
        'r' => &[1, 2, 3, 5],
        's' => &[2, 3, 4],
        't' => &[2, 3, 4, 5],
        'u' => &[1, 3, 6],
        'v' => &[1, 2, 3, 6],
        'w' => &[2, 4, 5, 6],
        'x' => &[1, 3, 4, 6],
        'y' => &[1, 3, 4, 5, 6],
        'z' => &[1, 3, 5, 6],
        _ => return None,
    })
}

/// Punctuation and signs (English braille, grade 1).
fn sign_dots(c: char) -> Option<&'static [u8]> {
    Some(match c {
        ',' => &[2],
        ';' => &[2, 3],
        ':' => &[2, 5],
        '.' => &[2, 5, 6],
        '!' => &[2, 3, 5],
        '?' => &[2, 3, 6],
        '\'' => &[3],
        '-' => &[3, 6],
        '(' | ')' => &[2, 3, 5, 6],
        '"' => &[2, 3, 6],
        '/' => &[3, 4],
        '@' => &[4, 7],
        '+' => &[2, 3, 5],
        '=' => &[2, 3, 5, 6],
        '*' => &[3, 5],
        '#' => &[3, 4, 5, 6],
        '_' => &[4, 5, 6],
        '%' => &[4, 5, 6, 1, 4],
        '&' => &[4, 1, 2, 3, 4, 6],
        '<' => &[4, 5, 1, 2, 6],
        '>' => &[4, 5, 3, 4, 5],
        _ => return None,
    })
}

/// A digit is the letters a to j after the number sign.
fn digit_letter(c: char) -> Option<char> {
    "jabcdefghi".chars().nth(c.to_digit(10)? as usize)
}

/// The text as braille cells with, for every cell, the index (in characters) of the source character it came from. A capital letter is
/// the capital sign and the letter, a number is the number sign and the digits (the sign is repeated after a space, not after a
/// comma or a point inside the number); a character that has no cell here comes out as the "unknown" cell (dots 456) rather than
/// being dropped.
pub fn translate(text: &str) -> (Vec<char>, Vec<usize>) {
    let (mut cells, mut from) = (Vec::new(), Vec::new());
    let mut in_number = false;
    for (i, c) in text.chars().enumerate() {
        let mut push = |ch: char| {
            cells.push(ch);
            from.push(i);
        };
        if c.is_ascii_digit() {
            if !in_number {
                push(cell(&[3, 4, 5, 6]));
                in_number = true;
            }
            if let Some(l) = digit_letter(c).and_then(letter_dots) {
                push(cell(l));
            }
            continue;
        }
        if in_number && (c == '.' || c == ',') {
            // a decimal point or a thousands comma stays inside the number
            push(cell(if c == '.' { &[2, 5, 6] } else { &[2] }));
            continue;
        }
        in_number = false;
        if c.is_whitespace() {
            push(cell(&[]));
        } else if c.is_ascii_uppercase() {
            push(cell(&[6]));
            if let Some(l) = letter_dots(c.to_ascii_lowercase()) {
                push(cell(l));
            }
        } else if let Some(l) = letter_dots(c) {
            push(cell(l));
        } else if let Some(l) = sign_dots(c) {
            // a few signs are two cells in braille (a prefix and a sign): the table holds them as one run
            let run: &[u8] = l;
            if run == [4, 5, 6, 1, 4] {
                push(cell(&[4, 5, 6]));
                push(cell(&[1, 4]));
            } else if run == [4, 1, 2, 3, 4, 6] {
                push(cell(&[4]));
                push(cell(&[1, 2, 3, 4, 6]));
            } else if run == [4, 5, 1, 2, 6] {
                push(cell(&[4, 5]));
                push(cell(&[1, 2, 6]));
            } else if run == [4, 5, 3, 4, 5] {
                push(cell(&[4, 5]));
                push(cell(&[3, 4, 5]));
            } else {
                push(cell(l));
            }
        } else {
            push(cell(&[4, 5, 6]));
        }
    }
    (cells, from)
}

/// What the reader's node says, as braille. A secret field is the same "password, 8 characters" words the speech uses, never its text.
pub fn for_node(n: &Node) -> String {
    translate(&n.speak()).0.into_iter().collect()
}

/// The cells of one line on a display of `width` cells, with the position the person has panned to.
pub struct Window {
    cells: Vec<char>,
    from: Vec<usize>,
    width: usize,
    start: usize,
}

impl Window {
    /// A display of `width` cells (at least one) showing the start of `text`.
    pub fn new(text: &str, width: usize) -> Window {
        let (cells, from) = translate(text);
        Window { cells, from, width: width.max(1), start: 0 }
    }

    /// The cells now on the display, padded with blank cells to the full width.
    pub fn cells(&self) -> String {
        let mut v: Vec<char> = self.cells.iter().skip(self.start).take(self.width).copied().collect();
        v.resize(self.width, cell(&[]));
        v.into_iter().collect()
    }

    /// Pan right by one display width (stops at the end); false when there is no more.
    pub fn pan_right(&mut self) -> bool {
        if self.start + self.width >= self.cells.len() {
            return false;
        }
        self.start = (self.start + self.width).min(self.cells.len().saturating_sub(1));
        true
    }

    /// Pan left by one display width (stops at the start); false when already there.
    pub fn pan_left(&mut self) -> bool {
        if self.start == 0 {
            return false;
        }
        self.start = self.start.saturating_sub(self.width);
        true
    }

    /// Cursor routing: the character of the source text under display cell `n` (counted from the left of the display), or `None` for a
    /// blank cell past the end of the line.
    pub fn route(&self, n: usize) -> Option<usize> {
        if n >= self.width {
            return None;
        }
        self.from.get(self.start + n).copied()
    }

    /// Pan so that the cell of source character `i` (the cursor) is on the display; the window starts at a multiple of the width.
    pub fn show_character(&mut self, i: usize) {
        if let Some(pos) = self.from.iter().position(|&f| f == i) {
            self.start = pos / self.width * self.width;
        }
    }

    pub fn total_cells(&self) -> usize {
        self.cells.len()
    }
}

/// What a connected display gives the reader: its width, a place to put cells, and the keys pressed. The core hands one over when a display is
/// connected (it is the only part that touches the device); a test uses a fake.
pub trait CellDisplay {
    fn width(&self) -> usize;
    fn put(&mut self, cells: &str) -> Result<(), String>;
    /// The keys pressed since the last call (never waits). A display with no keys gives none.
    fn keys(&mut self) -> Vec<Press> {
        Vec::new()
    }
}

/// What a key of the display asks the reader to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    PanLeft,
    PanRight,
    FocusPrev,
    FocusNext,
    /// The cursor goes to the character under display cell `n`.
    Route(usize),
}

/// What a `Press` did, for the screen reader to carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The display panned or has nothing more that way.
    Panned(bool),
    /// Move the focus to the previous or next element (the reader owns the focus; the display only asks).
    Focus { forward: bool },
    /// The cursor goes to this character of the shown line.
    Cursor(usize),
    /// A routing key above a blank cell.
    Nothing,
}

/// The line the reader last said, kept on the display. Words arrive with `announce` (the same words the speech says, a secret already spoken
/// as "password, 8 characters"); keys come in with `press`.
pub struct Session<D: CellDisplay> {
    display: D,
    window: Window,
    on: bool,
}

impl<D: CellDisplay> Session<D> {
    pub fn new(display: D, on: bool) -> Session<D> {
        let window = Window::new("", display.width());
        Session { display, window, on }
    }

    /// The person turned the braille output on or off; off blanks the display.
    pub fn set_on(&mut self, on: bool) -> Result<(), String> {
        self.on = on;
        if on {
            self.refresh()
        } else {
            let blank = cell(&[]).to_string().repeat(self.display.width());
            self.display.put(&blank)
        }
    }

    /// The reader says `words`: the display shows their start.
    pub fn announce(&mut self, words: &str) -> Result<(), String> {
        self.window = Window::new(words, self.display.width());
        if self.on { self.refresh() } else { Ok(()) }
    }

    pub fn press(&mut self, p: Press) -> Result<Outcome, String> {
        if !self.on {
            return Ok(Outcome::Nothing);
        }
        let out = match p {
            Press::PanLeft => Outcome::Panned(self.window.pan_left()),
            Press::PanRight => Outcome::Panned(self.window.pan_right()),
            Press::FocusPrev => Outcome::Focus { forward: false },
            Press::FocusNext => Outcome::Focus { forward: true },
            Press::Route(n) => self.window.route(n).map_or(Outcome::Nothing, Outcome::Cursor),
        };
        if matches!(out, Outcome::Panned(true)) {
            self.refresh()?;
        }
        Ok(out)
    }

    /// Take the keys pressed on the display since the last call and carry out each one on the line shown. The answers that are the reader's
    /// business (a focus move, a cursor) come back in order.
    pub fn poll(&mut self) -> Result<Vec<Outcome>, String> {
        let mut out = Vec::new();
        for p in self.display.keys() {
            let o = self.press(p)?;
            if o != Outcome::Nothing && !matches!(o, Outcome::Panned(_)) {
                out.push(o);
            }
        }
        Ok(out)
    }

    fn refresh(&mut self) -> Result<(), String> {
        let cells = self.window.cells();
        self.display.put(&cells)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::Role;

    fn s(t: &str) -> String {
        translate(t).0.into_iter().collect()
    }

    #[test]
    fn letters_digits_capitals_and_signs_use_the_published_patterns() {
        assert_eq!(s("abc"), "\u{2801}\u{2803}\u{2809}");
        // a number: the number sign then a to j; a capital: the capital sign (dot 6) then the letter
        assert_eq!(s("19"), "\u{283c}\u{2801}\u{280a}");
        assert_eq!(s("0"), "\u{283c}\u{281a}");
        assert_eq!(s("Hi"), "\u{2820}\u{2813}\u{280a}");
        assert_eq!(s("a b"), "\u{2801}\u{2800}\u{2803}");
        // the number sign comes back after a space, not inside 3.14
        assert_eq!(s("3.14").chars().filter(|&c| c == '\u{283c}').count(), 1);
        assert_eq!(s("1 2").chars().filter(|&c| c == '\u{283c}').count(), 2);
        assert_eq!(s("."), "\u{2832}");
        assert_eq!(s("?"), "\u{2826}");
        // a character with no cell is shown as the unknown cell, never dropped
        assert_eq!(s("\u{4e2d}"), "\u{2838}");
    }

    #[test]
    fn every_cell_knows_the_character_it_came_from() {
        let (cells, from) = translate("Ab 12");
        assert_eq!(cells.len(), from.len());
        assert_eq!(from, vec![0, 0, 1, 2, 3, 3, 4]);
    }

    #[test]
    fn a_line_longer_than_the_display_pans_and_routes_back_to_the_text() {
        let mut w = Window::new("hello world", 4);
        assert_eq!(w.cells().chars().count(), 4);
        assert_eq!(w.route(0), Some(0));
        assert_eq!(w.route(3), Some(3));
        assert!(w.pan_right());
        assert_eq!(w.route(0), Some(4));
        assert!(w.pan_right());
        assert!(!w.pan_right(), "the end");
        assert_eq!(w.cells().chars().count(), 4, "padded to the full width");
        assert!(w.pan_left() && w.pan_left());
        assert!(!w.pan_left(), "the start");
        w.show_character(9);
        assert_eq!(w.route(1), Some(9), "the cursor character is on the display");
        let short = Window::new("hi", 8);
        assert_eq!(short.route(5), None, "a blank cell past the end stands for no character");
        assert_eq!(Window::new("", 0).cells().chars().count(), 1, "a display has at least one cell");
    }

    #[test]
    fn a_secret_field_is_never_put_on_the_display() {
        let n = Node::field("Password", "hunter2", true);
        let b = for_node(&n);
        let plain: String = translate("hunter2").0.into_iter().collect();
        assert!(!b.contains(&plain), "the text of a secret is not on the display");
        assert!(!for_node(&Node::new("Save", Role::Button, crate::access::State::default())).is_empty());
    }
    struct Fake(usize, Vec<String>);
    impl CellDisplay for Fake {
        fn width(&self) -> usize {
            self.0
        }
        fn put(&mut self, cells: &str) -> Result<(), String> {
            self.1.push(cells.to_string());
            Ok(())
        }
    }

    #[test]
    fn the_session_shows_what_the_reader_says_pans_routes_and_blanks_when_turned_off() {
        let mut s = Session::new(Fake(4, vec![]), true);
        s.announce("hello world").unwrap();
        assert_eq!(s.display.1.last().unwrap(), &Window::new("hello world", 4).cells());
        assert_eq!(s.press(Press::PanRight).unwrap(), Outcome::Panned(true));
        assert_eq!(s.display.1.len(), 2, "a pan redraws");
        assert_eq!(s.press(Press::Route(0)).unwrap(), Outcome::Cursor(4));
        assert_eq!(s.press(Press::FocusNext).unwrap(), Outcome::Focus { forward: true });
        assert_eq!(s.press(Press::PanLeft).unwrap(), Outcome::Panned(true));
        assert_eq!(s.press(Press::PanLeft).unwrap(), Outcome::Panned(false));
        s.set_on(false).unwrap();
        assert_eq!(s.display.1.last().unwrap(), &"\u{2800}".repeat(4));
        let n = s.display.1.len();
        s.announce("quiet").unwrap();
        assert_eq!(s.display.1.len(), n, "off: nothing is written");
        assert_eq!(s.press(Press::PanRight).unwrap(), Outcome::Nothing);
        s.set_on(true).unwrap();
        assert_eq!(s.display.1.last().unwrap(), &Window::new("quiet", 4).cells());
    }
}
