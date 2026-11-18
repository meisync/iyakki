//! Keys to the bytes a terminal program expects (xterm style).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    BackTab,
    Backspace,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1 to F12.
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    fn code(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift
    }
}

/// The bytes to send for a key. `app_cursor` is the screen's application cursor keys mode (`CSI ? 1 h`).
pub fn encode(key: Key, m: Mods, app_cursor: bool) -> Vec<u8> {
    let csi = |fin: char| -> Vec<u8> {
        if m.any() {
            format!("\x1b[1;{}{}", m.code(), fin).into_bytes()
        } else if app_cursor {
            format!("\x1bO{fin}").into_bytes()
        } else {
            format!("\x1b[{fin}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m.any() {
            format!("\x1b[{};{}~", n, m.code()).into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    match key {
        Key::Up => csi('A'),
        Key::Down => csi('B'),
        Key::Right => csi('C'),
        Key::Left => csi('D'),
        Key::Home => csi('H'),
        Key::End => csi('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F(n @ 1..=4) => {
            let fin = (b'P' + n - 1) as char;
            if m.any() {
                format!("\x1b[1;{}{}", m.code(), fin).into_bytes()
            } else {
                format!("\x1bO{fin}").into_bytes()
            }
        }
        Key::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]),
        Key::F(_) => Vec::new(),
        Key::Enter => alt_prefix(m, b"\r".to_vec()),
        Key::Tab => alt_prefix(m, b"\t".to_vec()),
        Key::BackTab => b"\x1b[Z".to_vec(),
        Key::Backspace => alt_prefix(m, vec![0x7f]),
        Key::Esc => alt_prefix(m, vec![0x1b]),
        Key::Char(c) => {
            let mut out = Vec::new();
            if m.ctrl {
                let b = match c {
                    'a'..='z' => Some(c as u8 - b'a' + 1),
                    'A'..='Z' => Some(c as u8 - b'A' + 1),
                    '@' | ' ' | '2' => Some(0),
                    '[' | '3' => Some(0x1b),
                    '\\' | '4' => Some(0x1c),
                    ']' | '5' => Some(0x1d),
                    '^' | '6' => Some(0x1e),
                    '_' | '7' | '/' => Some(0x1f),
                    '8' | '?' => Some(0x7f),
                    _ => None,
                };
                if let Some(b) = b {
                    out.push(b);
                    return alt_prefix(m, out);
                }
            }
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            alt_prefix(m, out)
        }
    }
}

fn alt_prefix(m: Mods, bytes: Vec<u8>) -> Vec<u8> {
    if m.alt {
        let mut v = vec![0x1b];
        v.extend(bytes);
        v
    } else {
        bytes
    }
}

/// Pasted text for the program. In bracketed paste mode it is wrapped so the program can tell it from typing; either way
/// escape characters are removed so a paste cannot end the bracket early or send a control sequence.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let clean: String = text.chars().filter(|&c| c != '\x1b' && c != '\x03' && c != '\x04').collect();
    let clean = clean.replace("\r\n", "\r").replace('\n', "\r");
    let mut out = Vec::new();
    if bracketed {
        out.extend_from_slice(b"\x1b[200~");
    }
    out.extend_from_slice(clean.as_bytes());
    if bracketed {
        out.extend_from_slice(b"\x1b[201~");
    }
    out
}
