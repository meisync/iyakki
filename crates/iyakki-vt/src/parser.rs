//! The byte-stream side: UTF-8 decoding and the escape-sequence state machine (VT100 / ECMA-48 with the xterm extensions that
//! full-screen programs use). Unknown sequences are skipped whole, never printed, and every length is capped, so any byte stream
//! (a hostile server included) leaves the screen in a valid state.

use crate::screen::{Color, Screen, BLINK, BOLD, DIM, HIDDEN, ITALIC, REVERSE, STRIKE, UNDERLINE};

const MAX_PARAMS: usize = 24;
const MAX_PARAM_VALUE: usize = 65_535;
const MAX_OSC: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    /// After `ESC (`, `ESC )` and similar: one more byte names the set.
    Charset(u8),
    /// After `ESC #` and other intermediates that take one final byte, which is ignored.
    SkipOne,
    Csi,
    Osc,
    /// Inside `ESC P`, `ESC ^`, `ESC _`, `ESC X`: skipped up to the string terminator.
    Skip,
}

pub struct Parser {
    state: State,
    params: Vec<usize>,
    cur: Option<usize>,
    private: Option<u8>,
    inter: Vec<u8>,
    osc: Vec<u8>,
    esc_in_string: bool,
    utf8: Vec<u8>,
    utf8_need: usize,
    /// Bytes of a CSI sequence that was malformed are skipped to its final byte.
    bad: bool,
}

impl Default for Parser {
    fn default() -> Parser {
        Parser::new()
    }
}

impl Parser {
    pub fn new() -> Parser {
        Parser { state: State::Ground, params: Vec::new(), cur: None, private: None, inter: Vec::new(), osc: Vec::new(), esc_in_string: false, utf8: Vec::new(), utf8_need: 0, bad: false }
    }

    /// Feed bytes from the program to the screen.
    pub fn feed(&mut self, s: &mut Screen, bytes: &[u8]) {
        for &b in bytes {
            self.byte(s, b);
        }
    }

    fn byte(&mut self, s: &mut Screen, b: u8) {
        // C0 controls act in every state except inside strings (and are never part of a sequence's parameters)
        if self.utf8_need > 0 {
            if b & 0xc0 == 0x80 {
                self.utf8.push(b);
                self.utf8_need -= 1;
                if self.utf8_need == 0 {
                    let ch = std::str::from_utf8(&self.utf8).ok().and_then(|t| t.chars().next()).unwrap_or('\u{fffd}');
                    self.utf8.clear();
                    self.glyph(s, ch);
                }
                return;
            }
            // broken sequence: show one replacement mark and treat this byte afresh
            self.utf8.clear();
            self.utf8_need = 0;
            self.glyph(s, '\u{fffd}');
        }
        match self.state {
            State::Osc | State::Skip => self.string_byte(s, b),
            _ if b == 0x1b => {
                self.state = State::Escape;
            }
            _ if b == 0x18 || b == 0x1a => {
                self.state = State::Ground; // CAN and SUB end any sequence
                self.clear();
            }
            _ if b < 0x20 => self.control(s, b),
            State::Ground => {
                if b < 0x80 {
                    self.glyph(s, b as char);
                } else if (0xc2..=0xdf).contains(&b) {
                    self.start_utf8(b, 1);
                } else if (0xe0..=0xef).contains(&b) {
                    self.start_utf8(b, 2);
                } else if (0xf0..=0xf4).contains(&b) {
                    self.start_utf8(b, 3);
                } else {
                    self.glyph(s, '\u{fffd}');
                }
            }
            State::Escape => self.escape(s, b),
            State::Charset(which) => {
                if which == b'(' {
                    s.line_draw = b == b'0';
                }
                self.state = State::Ground;
            }
            State::SkipOne => self.state = State::Ground,
            State::Csi => self.csi_byte(s, b),
        }
    }

    fn start_utf8(&mut self, b: u8, more: usize) {
        self.utf8.clear();
        self.utf8.push(b);
        self.utf8_need = more;
    }

    fn glyph(&mut self, s: &mut Screen, c: char) {
        s.print(c);
    }

    fn clear(&mut self) {
        self.params.clear();
        self.cur = None;
        self.private = None;
        self.inter.clear();
        self.bad = false;
    }

    fn control(&mut self, s: &mut Screen, b: u8) {
        match b {
            0x07 => {} // bell: the display decides what to do
            0x08 => s.backspace(),
            0x09 => s.tab(1),
            0x0a..=0x0c => {
                s.linefeed();
                if s.newline_mode {
                    s.carriage_return();
                }
            }
            0x0d => s.carriage_return(),
            0x0e => s.line_draw = true,  // SO: the graphics set
            0x0f => s.line_draw = false, // SI
            _ => {}
        }
    }

    fn escape(&mut self, s: &mut Screen, b: u8) {
        self.state = State::Ground;
        match b {
            b'[' => {
                self.clear();
                self.state = State::Csi;
            }
            b']' => {
                self.osc.clear();
                self.esc_in_string = false;
                self.state = State::Osc;
            }
            b'P' | b'^' | b'_' | b'X' => {
                self.esc_in_string = false;
                self.state = State::Skip;
            }
            b'(' | b')' | b'*' | b'+' => self.state = State::Charset(b),
            b'#' | b' ' | b'%' => self.state = State::SkipOne,
            b'7' => s.save_cursor(),
            b'8' => s.restore_cursor(),
            b'D' => s.linefeed(),
            b'E' => {
                s.linefeed();
                s.carriage_return();
            }
            b'M' => s.reverse_index(),
            b'H' => s.set_tab(),
            b'c' => s.reset(),
            b'=' => s.app_keypad = true,
            b'>' => s.app_keypad = false,
            _ => {}
        }
    }

    fn string_byte(&mut self, s: &mut Screen, b: u8) {
        let end = b == 0x07 || (self.esc_in_string && b == b'\\');
        if b == 0x1b {
            self.esc_in_string = true;
            return;
        }
        if end {
            if self.state == State::Osc {
                self.finish_osc(s);
            }
            self.state = State::Ground;
            self.esc_in_string = false;
            return;
        }
        if self.esc_in_string {
            // ESC followed by something other than `\` ends the string and starts a new sequence
            self.esc_in_string = false;
            self.state = State::Ground;
            self.escape(s, b);
            return;
        }
        if self.state == State::Osc && self.osc.len() < MAX_OSC && b >= 0x20 {
            self.osc.push(b);
        }
    }

    fn finish_osc(&mut self, s: &mut Screen) {
        let text = String::from_utf8_lossy(&self.osc).into_owned();
        let (num, rest) = text.split_once(';').unwrap_or((text.as_str(), ""));
        if matches!(num, "0" | "2") {
            // a title is shown, so it may not carry control characters
            s.title = rest.chars().filter(|c| !c.is_control()).take(120).collect();
            s.generation = s.generation.wrapping_add(1);
        }
        self.osc.clear();
    }

    fn csi_byte(&mut self, s: &mut Screen, b: u8) {
        match b {
            b'0'..=b'9' => {
                let d = (b - b'0') as usize;
                self.cur = Some((self.cur.unwrap_or(0) * 10 + d).min(MAX_PARAM_VALUE));
            }
            b';' | b':' => {
                if self.params.len() < MAX_PARAMS {
                    self.params.push(self.cur.take().unwrap_or(0));
                } else {
                    self.bad = true;
                }
                self.cur = None;
            }
            b'<' | b'=' | b'>' | b'?' => self.private = Some(b),
            0x20..=0x2f => self.inter.push(b),
            0x40..=0x7e => {
                if self.cur.is_some() || !self.params.is_empty() {
                    if self.params.len() < MAX_PARAMS {
                        self.params.push(self.cur.take().unwrap_or(0));
                    }
                }
                let ok = !self.bad && self.inter.is_empty();
                if ok {
                    let params = std::mem::take(&mut self.params);
                    let private = self.private;
                    self.dispatch(s, b, &params, private);
                }
                self.clear();
                self.state = State::Ground;
            }
            _ => {
                // 0x7f and anything else odd: ignored inside the sequence
            }
        }
    }

    fn dispatch(&mut self, s: &mut Screen, fin: u8, p: &[usize], private: Option<u8>) {
        let arg = |i: usize, dflt: usize| -> usize {
            match p.get(i) {
                Some(&0) | None => dflt,
                Some(&v) => v,
            }
        };
        if let Some(pm) = private {
            match (pm, fin) {
                (b'?', b'h') => p.iter().for_each(|&m| set_private(s, m, true)),
                (b'?', b'l') => p.iter().for_each(|&m| set_private(s, m, false)),
                (b'>', b'c') => s.replies.extend_from_slice(b"\x1b[>0;0;0c"),
                _ => {}
            }
            return;
        }
        match fin {
            b'A' => s.move_by(-(arg(0, 1) as isize), 0),
            b'B' | b'e' => s.move_by(arg(0, 1) as isize, 0),
            b'C' | b'a' => s.move_by(0, arg(0, 1) as isize),
            b'D' => s.move_by(0, -(arg(0, 1) as isize)),
            b'E' => {
                s.move_by(arg(0, 1) as isize, 0);
                s.set_x(0);
            }
            b'F' => {
                s.move_by(-(arg(0, 1) as isize), 0);
                s.set_x(0);
            }
            b'G' | b'`' => s.set_x(arg(0, 1) - 1),
            b'd' => s.set_y(arg(0, 1) - 1),
            b'H' | b'f' => s.move_to(arg(0, 1) - 1, arg(1, 1) - 1),
            b'I' => s.tab(arg(0, 1)),
            b'Z' => s.back_tab(arg(0, 1)),
            b'J' => s.erase_display(p.first().copied().unwrap_or(0)),
            b'K' => s.erase_line(p.first().copied().unwrap_or(0)),
            b'L' => s.insert_lines(arg(0, 1)),
            b'M' => s.delete_lines(arg(0, 1)),
            b'@' => s.insert_chars(arg(0, 1)),
            b'P' => s.delete_chars(arg(0, 1)),
            b'X' => s.erase_chars(arg(0, 1)),
            b'S' => s.scroll_up(arg(0, 1)),
            b'T' => s.scroll_down(arg(0, 1)),
            b'g' => s.clear_tab(p.first().copied().unwrap_or(0) == 3),
            b'm' => sgr(s, p),
            b'r' => {
                let top = arg(0, 1) - 1;
                let bottom = if p.get(1).copied().unwrap_or(0) == 0 { s.rows() - 1 } else { arg(1, 1) - 1 };
                s.set_region(top, bottom);
            }
            b's' => s.save_cursor(),
            b'u' => s.restore_cursor(),
            b'h' => set_ansi(s, p, true),
            b'l' => set_ansi(s, p, false),
            b'n' => match p.first().copied().unwrap_or(0) {
                5 => s.replies.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let row = if s.origin { s.y.saturating_sub(s.region().0) } else { s.y } + 1;
                    s.replies.extend_from_slice(format!("\x1b[{};{}R", row, s.x + 1).as_bytes());
                }
                _ => {}
            },
            b'c' => s.replies.extend_from_slice(b"\x1b[?62;22c"),
            _ => {}
        }
    }
}

fn set_ansi(s: &mut Screen, p: &[usize], on: bool) {
    for &m in p {
        match m {
            4 => s.insert = on,
            20 => s.newline_mode = on,
            _ => {}
        }
    }
}

fn set_private(s: &mut Screen, mode: usize, on: bool) {
    match mode {
        1 => s.app_cursor_keys = on,
        6 => {
            s.origin = on;
            s.move_to(0, 0);
        }
        7 => s.autowrap = on,
        25 => s.cursor_visible = on,
        47 | 1047 => {
            if on {
                s.enter_alt(false)
            } else {
                s.leave_alt(false)
            }
        }
        1049 => {
            if on {
                s.enter_alt(true)
            } else {
                s.leave_alt(true)
            }
        }
        2004 => s.bracketed_paste = on,
        _ => {} // mouse reporting, focus events and the like are not offered: the program is told nothing it did not ask for
    }
    s.generation = s.generation.wrapping_add(1);
}

fn sgr(s: &mut Screen, p: &[usize]) {
    if p.is_empty() {
        s.pen = Default::default();
        return;
    }
    let mut i = 0;
    while i < p.len() {
        let v = p[i];
        match v {
            0 => s.pen = Default::default(),
            1 => s.pen.attrs |= BOLD,
            2 => s.pen.attrs |= DIM,
            3 => s.pen.attrs |= ITALIC,
            4 => s.pen.attrs |= UNDERLINE,
            5 | 6 => s.pen.attrs |= BLINK,
            7 => s.pen.attrs |= REVERSE,
            8 => s.pen.attrs |= HIDDEN,
            9 => s.pen.attrs |= STRIKE,
            22 => s.pen.attrs &= !(BOLD | DIM),
            23 => s.pen.attrs &= !ITALIC,
            24 => s.pen.attrs &= !UNDERLINE,
            25 => s.pen.attrs &= !BLINK,
            27 => s.pen.attrs &= !REVERSE,
            28 => s.pen.attrs &= !HIDDEN,
            29 => s.pen.attrs &= !STRIKE,
            30..=37 => s.pen.fg = Color::Indexed((v - 30) as u8),
            38 | 48 => {
                let (color, used) = extended(&p[i + 1..]);
                if let Some(c) = color {
                    if v == 38 {
                        s.pen.fg = c
                    } else {
                        s.pen.bg = c
                    }
                }
                i += used;
            }
            39 => s.pen.fg = Color::Default,
            40..=47 => s.pen.bg = Color::Indexed((v - 40) as u8),
            49 => s.pen.bg = Color::Default,
            90..=97 => s.pen.fg = Color::Indexed((v - 90 + 8) as u8),
            100..=107 => s.pen.bg = Color::Indexed((v - 100 + 8) as u8),
            _ => {}
        }
        i += 1;
    }
}

/// `5;n` or `2;r;g;b` after a 38 or 48: the colour and how many parameters were used.
fn extended(p: &[usize]) -> (Option<Color>, usize) {
    match p.first() {
        Some(5) => (p.get(1).map(|&n| Color::Indexed(n.min(255) as u8)), 2.min(p.len())),
        Some(2) if p.len() >= 4 => (Some(Color::Rgb(p[1].min(255) as u8, p[2].min(255) as u8, p[3].min(255) as u8)), 4),
        _ => (None, p.len()),
    }
}
