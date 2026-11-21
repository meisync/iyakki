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
    translate_grade(text, Grade::One)
}

/// Uncontracted (grade 1: one cell a letter) or contracted (grade 2: the word signs, the strong and the group signs of English braille).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grade {
    #[default]
    One,
    Two,
}

impl Grade {
    pub fn from_word(w: &str) -> Option<Grade> {
        match w.trim().to_ascii_lowercase().as_str() {
            "1" | "one" | "grade1" | "uncontracted" => Some(Grade::One),
            "2" | "two" | "grade2" | "contracted" => Some(Grade::Two),
            _ => None,
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            Grade::One => "uncontracted",
            Grade::Two => "contracted",
        }
    }
}

/// A letter word on its own: the one-letter word signs and the whole-word signs (strong word signs).
fn word_sign(w: &str) -> Option<&'static [u8]> {
    Some(match w {
        "but" => &[1, 2],
        "can" => &[1, 4],
        "do" => &[1, 4, 5],
        "every" => &[1, 5],
        "from" => &[1, 2, 4],
        "go" => &[1, 2, 4, 5],
        "have" => &[1, 2, 5],
        "just" => &[2, 4, 5],
        "knowledge" => &[1, 3],
        "like" => &[1, 2, 3],
        "more" => &[1, 3, 4],
        "not" => &[1, 3, 4, 5],
        "people" => &[1, 2, 3, 4],
        "quite" => &[1, 2, 3, 4, 5],
        "rather" => &[1, 2, 3, 5],
        "so" => &[2, 3, 4],
        "that" => &[2, 3, 4, 5],
        "us" => &[1, 3, 6],
        "very" => &[1, 2, 3, 6],
        "will" => &[2, 4, 5, 6],
        "it" => &[1, 3, 4, 6],
        "you" => &[1, 3, 4, 5, 6],
        "as" => &[1, 3, 5, 6],
        "and" => &[1, 2, 3, 4, 6],
        "for" => &[1, 2, 3, 4, 5, 6],
        "of" => &[1, 2, 3, 5, 6],
        "the" => &[2, 3, 4, 6],
        "with" => &[2, 3, 4, 5, 6],
        "child" => &[1, 6],
        "shall" => &[1, 4, 6],
        "this" => &[1, 4, 5, 6],
        "which" => &[1, 5, 6],
        "out" => &[1, 2, 5, 6],
        "still" => &[3, 4],
        // the lower word signs (a word on its own)
        "be" => &[2, 3],
        "enough" => &[2, 6],
        "were" => &[2, 3, 5, 6],
        "his" => &[2, 3, 6],
        "in" => &[3, 5],
        "was" => &[3, 5, 6],
        _ => return None,
    })
}

/// The signs used inside a word, longest first. `ing` never starts a word.
const GROUPS: [(&str, &[u8]); 15] = [
    ("and", &[1, 2, 3, 4, 6]),
    ("for", &[1, 2, 3, 4, 5, 6]),
    ("the", &[2, 3, 4, 6]),
    ("with", &[2, 3, 4, 5, 6]),
    ("ing", &[3, 4, 6]),
    ("ch", &[1, 6]),
    ("gh", &[1, 2, 6]),
    ("sh", &[1, 4, 6]),
    ("th", &[1, 4, 5, 6]),
    ("wh", &[1, 5, 6]),
    ("ed", &[1, 2, 4, 6]),
    ("er", &[1, 2, 4, 5, 6]),
    ("ou", &[1, 2, 5, 6]),
    ("ow", &[2, 4, 6]),
    ("ar", &[3, 4, 5]),
];

/// Where in a word a sign may stand.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spot {
    /// Anywhere in the word.
    Any,
    /// Not as the first letters of the word (the final-letter signs, `ble`).
    NotStart,
    /// Only at the start of the word (the prefixes).
    Start,
    /// Only between the first and last letter (the lower group signs `ea bb cc ff gg`).
    Middle,
}

/// The signs of one or two cells that are not in [`GROUPS`]: the lower group signs, the prefixes and the final-letter signs.
const MORE_SIGNS: [(&str, &[&[u8]], Spot); 24] = [
    ("ound", &[&[4, 6], &[1, 4, 5]], Spot::NotStart),
    ("ance", &[&[4, 6], &[1, 5]], Spot::NotStart),
    ("sion", &[&[4, 6], &[1, 3, 4, 5]], Spot::NotStart),
    ("less", &[&[4, 6], &[2, 3, 4]], Spot::NotStart),
    ("ount", &[&[4, 6], &[2, 3, 4, 5]], Spot::NotStart),
    ("ence", &[&[5, 6], &[1, 5]], Spot::NotStart),
    ("tion", &[&[5, 6], &[1, 3, 4, 5]], Spot::NotStart),
    ("ness", &[&[5, 6], &[2, 3, 4]], Spot::NotStart),
    ("ment", &[&[5, 6], &[2, 3, 4, 5]], Spot::NotStart),
    ("ong", &[&[5, 6], &[1, 2, 4, 5]], Spot::NotStart),
    ("ful", &[&[5, 6], &[1, 2, 3]], Spot::NotStart),
    ("ity", &[&[5, 6], &[1, 3, 4, 5, 6]], Spot::NotStart),
    ("ble", &[&[3, 4, 5, 6]], Spot::NotStart),
    ("com", &[&[3, 6]], Spot::Start),
    ("con", &[&[2, 5]], Spot::Start),
    ("dis", &[&[2, 5, 6]], Spot::Start),
    ("ea", &[&[2]], Spot::Middle),
    ("bb", &[&[2, 3]], Spot::Middle),
    ("cc", &[&[2, 5]], Spot::Middle),
    ("ff", &[&[2, 3, 5]], Spot::Middle),
    ("gg", &[&[2, 3, 5, 6]], Spot::Middle),
    ("en", &[&[2, 6]], Spot::Any),
    ("in", &[&[3, 5]], Spot::Any),
    ("st", &[&[3, 4]], Spot::Any),
];

/// The shortforms (words written with a few letters and signs): the word, and its spelling as letters with a group in brackets.
const SHORTFORMS: [(&str, &str); 66] = [
    ("about", "ab"), ("above", "abv"), ("according", "ac"), ("across", "acr"), ("after", "af"), ("afternoon", "afn"), ("afterward", "afw"),
    ("again", "ag"), ("against", "agst"), ("also", "al"), ("almost", "alm"), ("already", "alr"), ("altogether", "alt"), ("always", "alw"),
    ("before", "bef"), ("behind", "beh"), ("below", "bel"), ("beneath", "ben"), ("beside", "bes"), ("between", "bet"), ("beyond", "bey"),
    ("blind", "bl"), ("braille", "brl"), ("children", "chn"), ("conceive", "concv"), ("conceiving", "concvg"), ("could", "cd"),
    ("deceive", "dcv"), ("deceiving", "dcvg"), ("declare", "dcl"), ("declaring", "dclg"), ("either", "ei"), ("first", "f(st)"),
    ("friend", "fr"), ("good", "gd"), ("great", "grt"), ("herself", "h(er)f"), ("him", "hm"), ("himself", "hmf"), ("immediate", "imm"),
    ("letter", "lr"), ("little", "ll"), ("much", "m(ch)"), ("must", "m(st)"), ("myself", "myf"), ("necessary", "nec"), ("neither", "nei"),
    ("paid", "pd"), ("perceive", "percv"), ("perceiving", "percvg"), ("perhaps", "p(er)h"), ("quick", "qk"), ("receive", "rcv"),
    ("receiving", "rcvg"), ("rejoice", "rjc"), ("rejoicing", "rjcg"), ("said", "sd"), ("should", "(sh)d"), ("such", "s(ch)"),
    ("today", "td"), ("together", "tgr"), ("tomorrow", "tm"), ("tonight", "tn"), ("would", "wd"), ("your", "yr"), ("yourself", "yrf"),
];

/// The cells of a shortform's spelling.
fn spell_shortform(spelling: &str) -> Option<Vec<char>> {
    let mut out = Vec::new();
    let b: Vec<char> = spelling.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '(' {
            let close = b[i..].iter().position(|&c| c == ')')? + i;
            let g: String = b[i + 1..close].iter().collect();
            let dots = GROUPS.iter().find(|(n, _)| *n == g).map(|(_, d)| *d).or_else(|| MORE_SIGNS.iter().find(|(n, _, _)| *n == g).map(|(_, d, _)| d[0]))?;
            out.push(cell(dots));
            i = close + 1;
        } else {
            out.push(cell(letter_dots(b[i])?));
            i += 1;
        }
    }
    Some(out)
}

/// One word of letters (lower case, optionally one capital first) in grade 2: cells with the index of the character each came from
/// (`at` is the index of the word's first character). `None` when the word is written some other way (all capitals, a capital in the
/// middle, an apostrophe): the caller then writes it letter by letter.
fn contract_word(chars: &[char], at: usize) -> Option<(Vec<char>, Vec<usize>)> {
    if chars.is_empty() || chars.iter().any(|c| !c.is_ascii_alphabetic()) || chars[1..].iter().any(|c| c.is_ascii_uppercase()) {
        return None;
    }
    let lower: String = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let (mut cells, mut from) = (Vec::new(), Vec::new());
    if chars[0].is_ascii_uppercase() {
        cells.push(cell(&[6]));
        from.push(at);
    }
    if let Some(d) = word_sign(&lower) {
        cells.push(cell(d));
        from.push(at);
        return Some((cells, from));
    }
    if let Some(sp) = SHORTFORMS.iter().find(|(w, _)| *w == lower).and_then(|(_, sp)| spell_shortform(sp)) {
        for c in sp {
            cells.push(c);
            from.push(at);
        }
        return Some((cells, from));
    }
    let b: Vec<char> = lower.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let rest: String = b[i..].iter().collect();
        let hit = GROUPS.iter().find(|(g, _)| rest.starts_with(g) && !(*g == "ing" && i == 0) && !(g.len() == b.len() && i == 0));
        let more = MORE_SIGNS.iter().find(|(g, _, spot)| {
            rest.starts_with(g)
                && match spot {
                    Spot::Any => g.len() != b.len(),
                    Spot::NotStart => i > 0,
                    Spot::Start => i == 0 && g.len() < b.len(),
                    Spot::Middle => i > 0 && i + g.len() < b.len(),
                }
        });
        // a longer sign wins over a shorter one that starts at the same place
        let take_more = more.is_some_and(|(g, _, _)| hit.is_none_or(|(h, _)| g.len() > h.len()));
        if take_more {
            let (g, cs, _) = more?;
            for d in *cs {
                cells.push(cell(d));
                from.push(at + i);
            }
            i += g.len();
        } else if let Some((g, d)) = hit {
            cells.push(cell(d));
            from.push(at + i);
            i += g.len();
        } else {
            cells.push(cell(letter_dots(b[i])?));
            from.push(at + i);
            i += 1;
        }
    }
    Some((cells, from))
}

/// Like [`translate`] in the chosen grade. In grade 2 every cell still knows the character it came from (a contraction knows its first).
pub fn translate_grade(text: &str, grade: Grade) -> (Vec<char>, Vec<usize>) {
    let (mut cells, mut from) = (Vec::new(), Vec::new());
    let mut in_number = false;
    let all: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < all.len() {
        let c = all[i];
        if grade == Grade::Two && c.is_ascii_alphabetic() && (i == 0 || !(all[i - 1].is_ascii_alphanumeric() || all[i - 1] == '\'')) {
            let mut j = i;
            while j < all.len() && (all[j].is_ascii_alphanumeric() || all[j] == '\'') {
                j += 1;
            }
            if let Some((cs, fs)) = contract_word(&all[i..j], i) {
                cells.extend(cs);
                from.extend(fs);
                in_number = false;
                i = j;
                continue;
            }
        }
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
            i += 1;
            continue;
        }
        if in_number && (c == '.' || c == ',') {
            // a decimal point or a thousands comma stays inside the number
            push(cell(if c == '.' { &[2, 5, 6] } else { &[2] }));
            i += 1;
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
        i += 1;
    }
    (cells, from)
}

/// What the reader's node says, as braille. A secret field is the same "password, 8 characters" words the speech uses, never its text.
pub fn for_node(n: &Node) -> String {
    translate(&n.speak()).0.into_iter().collect()
}

/// Like [`for_node`] in the chosen grade.
pub fn for_node_grade(n: &Node, grade: Grade) -> String {
    translate_grade(&n.speak(), grade).0.into_iter().collect()
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
        Window::with_grade(text, width, Grade::One)
    }

    /// Like [`Window::new`] in the chosen grade.
    pub fn with_grade(text: &str, width: usize, grade: Grade) -> Window {
        let (cells, from) = translate_grade(text, grade);
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

/// Cursor routing in a text field: the character of the field's text that a routing key points at. `spoken_index` is the character of the line
/// the reader said for the node (`Node::speak`, which is what the display shows). `None` when the node is not a text field that shows its text
/// (a secret field never does); a key over the name or the role word puts the caret at the start, one past the end at the end.
pub fn field_caret(node: &Node, spoken_index: usize) -> Option<usize> {
    if node.role != crate::access::Role::Field || node.is_secret() {
        return None;
    }
    let value = node.state.value_text().filter(|v| !v.is_empty())?;
    // the value is the third part of "name, role, value" and the state words (if any) come after it: count from the front
    let name = if node.name.is_empty() { "unnamed".chars().count() } else { node.name.chars().count() };
    let start = name + 2 + node.role.word(false).chars().count() + 2;
    let len = value.chars().count();
    Some(spoken_index.saturating_sub(start).min(len))
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
    grade: Grade,
    said: String,
}

impl<D: CellDisplay> Session<D> {
    pub fn new(display: D, on: bool) -> Session<D> {
        let window = Window::new("", display.width());
        Session { display, window, on, grade: Grade::One, said: String::new() }
    }

    /// The person chose uncontracted or contracted braille: the line on the display is written again in that grade.
    pub fn set_grade(&mut self, grade: Grade) -> Result<(), String> {
        if grade == self.grade {
            return Ok(());
        }
        self.grade = grade;
        self.window = Window::with_grade(&self.said, self.display.width(), grade);
        if self.on { self.refresh() } else { Ok(()) }
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
        self.said = words.to_string();
        self.window = Window::with_grade(words, self.display.width(), self.grade);
        if self.on { self.refresh() } else { Ok(()) }
    }

    /// The line the reader last said and the display shows.
    pub fn line(&self) -> &str {
        &self.said
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

    fn g2(t: &str) -> String {
        translate_grade(t, Grade::Two).0.into_iter().collect()
    }

    #[test]
    fn grade_two_uses_word_signs_strong_signs_and_group_signs_and_grade_one_is_unchanged() {
        // a lone letter-word is its word sign: "x" is it, "but" is b
        assert_eq!(g2("but"), "\u{2803}");
        assert_eq!(g2("the and for of with"), "\u{282e}\u{2800}\u{282f}\u{2800}\u{283f}\u{2800}\u{2837}\u{2800}\u{283e}");
        // group signs inside a word: "church" is ch u r ch, "thing" is th ing
        assert_eq!(g2("church"), format!("{}{}{}{}", '\u{2821}', '\u{2825}', '\u{2817}', '\u{2821}'));
        assert_eq!(g2("thing"), format!("{}{}", '\u{2839}', '\u{282c}'));
        // a capital first letter keeps its sign, the sign still counts
        assert_eq!(g2("The"), "\u{2820}\u{282e}");
        // fewer cells than grade one
        let t = "the children shall stand with you";
        assert!(translate_grade(t, Grade::Two).0.len() < translate_grade(t, Grade::One).0.len());
        // grade one is what translate gives
        assert_eq!(translate_grade(t, Grade::One), translate(t));
    }

    #[test]
    fn grade_two_has_the_lower_word_signs_the_shortforms_the_prefixes_and_the_final_letter_signs_where_they_may_stand() {
        // lower word signs stand alone: be (dots 23), his (236), in (35), was (356)
        assert_eq!(g2("be"), cell(&[2, 3]).to_string());
        assert_eq!(g2("was"), cell(&[3, 5, 6]).to_string());
        assert_eq!(g2("his in"), format!("{}{}{}", cell(&[2, 3, 6]), cell(&[]), cell(&[3, 5])));
        // shortforms: "about" is ab, "first" is f and the st sign, "should" is the sh sign and d
        assert_eq!(g2("about"), translate("ab").0.into_iter().collect::<String>());
        assert_eq!(g2("first"), format!("{}{}", cell(&[1, 2, 4]), cell(&[3, 4])));
        assert_eq!(g2("should"), format!("{}{}", cell(&[1, 4, 6]), cell(&[1, 4, 5])));
        assert_eq!(g2("Braille"), format!("{}{}", cell(&[6]), translate("brl").0.into_iter().collect::<String>()));
        // a shortform is only a whole word: "aboutface" is spelled out
        assert_ne!(g2("aboutface"), g2("about") + &g2("face"));
        // prefixes only at the start: "common" starts with the com sign, "income" does not
        assert!(g2("common").starts_with(cell(&[3, 6])));
        assert!(!g2("income").contains(cell(&[3, 6])));
        assert!(g2("discover").starts_with(cell(&[2, 5, 6])));
        // final-letter signs are two cells and never start a word: "mention" ends with dots 56 then 1345, "tional" does not start with them
        assert!(g2("mention").ends_with(&format!("{}{}", cell(&[5, 6]), cell(&[1, 3, 4, 5]))));
        assert!(g2("table").ends_with(cell(&[3, 4, 5, 6])), "ble at the end");
        assert!(!g2("blame").starts_with(cell(&[3, 4, 5, 6])), "ble never starts a word");
        assert!(g2("kindness").ends_with(&format!("{}{}", cell(&[5, 6]), cell(&[2, 3, 4]))));
        // lower group signs only between letters: "bead" has ea, "eat" does not
        assert!(g2("bead").contains(cell(&[2])));
        assert!(!g2("eat").contains(cell(&[2])));
        // every cell still knows the character it came from, so routing works
        let (cells, from) = translate_grade("mention", Grade::Two);
        assert_eq!(cells.len(), from.len());
        assert!(from.windows(2).all(|w| w[0] <= w[1]));
        // grade two is never longer than grade one for ordinary text
        for t in ["Everything happens for a reason", "the friend would receive a good letter", "mention the conversation about the condition"] {
            assert!(translate_grade(t, Grade::Two).0.len() <= translate(t).0.len(), "{t}");
        }
    }

    #[test]
    fn a_routing_key_over_a_text_field_gives_the_character_of_its_text_and_never_for_a_secret_or_a_button() {
        use crate::access::{Node, Role, State};
        let f = Node::field("Name", "Ada Lovelace", false);
        // "Name, text field, Ada Lovelace": the value starts after "Name, " and the role word
        let start = "Name, ".chars().count() + f.role.word(false).chars().count() + 2;
        assert_eq!(field_caret(&f, 0), Some(0), "over the name: the start");
        assert_eq!(field_caret(&f, start), Some(0));
        assert_eq!(field_caret(&f, start + 4), Some(4));
        assert_eq!(field_caret(&f, start + 500), Some(12), "past the end: the end");
        assert_eq!(field_caret(&Node::field("Password", "hunter2", true), 10), None);
        assert_eq!(field_caret(&Node::field("Empty", "", false), 3), None);
        assert_eq!(field_caret(&Node::new("Save", Role::Button, State::default()), 2), None);
    }

    #[test]
    fn grade_two_leaves_numbers_capitals_apostrophes_and_unknown_characters_alone_and_routes_back() {
        // a letter after a digit is not a word sign ("3d" is not "3 do"), "don't" is not "do n't", all capitals are written letter by letter
        assert_eq!(g2("3d"), translate("3d").0.into_iter().collect::<String>());
        assert_eq!(g2("don't"), translate("don't").0.into_iter().collect::<String>());
        assert_eq!(g2("US"), translate("US").0.into_iter().collect::<String>());
        assert_eq!(g2("\u{e9}"), translate("\u{e9}").0.into_iter().collect::<String>());
        // every cell still knows its character; a contraction knows its first
        let (cells, from) = translate_grade("the church", Grade::Two);
        assert_eq!(cells.len(), from.len());
        assert_eq!(from[0], 0);
        assert_eq!(from[2], 4, "the first cell of 'church' is the ch at index 4");
        assert!(from.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn a_session_writes_the_line_again_when_the_grade_changes_and_a_secret_stays_hidden() {
        struct D(std::rc::Rc<std::cell::RefCell<Vec<String>>>);
        impl CellDisplay for D {
            fn width(&self) -> usize {
                20
            }
            fn put(&mut self, cells: &str) -> Result<(), String> {
                self.0.borrow_mut().push(cells.to_string());
                Ok(())
            }
        }
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut s = Session::new(D(log.clone()), true);
        s.announce("the children").unwrap();
        let one = log.borrow().last().cloned().unwrap();
        s.set_grade(Grade::Two).unwrap();
        let two = log.borrow().last().cloned().unwrap();
        assert_ne!(one, two);
        assert!(two.starts_with('\u{282e}'), "the word sign for 'the' first");
        // the same grade again writes nothing new
        let n = log.borrow().len();
        s.set_grade(Grade::Two).unwrap();
        assert_eq!(log.borrow().len(), n);
        // a password field says its count, in either grade
        let pw = Node::field("Password", "hunter2", true);
        assert_eq!(for_node_grade(&pw, Grade::Two), for_node_grade(&Node::field("Password", "zzzzzzz", true), Grade::Two));
        assert_eq!(Grade::from_word("Contracted"), Some(Grade::Two));
    }
}
