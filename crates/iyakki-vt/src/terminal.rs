//! A terminal on any byte link: the parser and the screen behind a [`Link`] (a pseudo-terminal on Linux, a fake in tests). Everything
//! the tile does with a session goes through here, so it can be tested without a process.

use crate::keys::{self, Key, Mods};
use crate::{Parser, Screen};

/// What a read found.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum LinkState {
    Open,
    Closed,
}

/// The other end of a terminal: the program's output comes in, keys go out.
pub trait Link {
    /// Append what has arrived (without waiting).
    fn read(&mut self, out: &mut Vec<u8>) -> LinkState;
    fn write(&mut self, bytes: &[u8]);
    fn resize(&mut self, cols: u16, rows: u16);
    /// The exit code once the program has ended.
    fn exit_code(&mut self) -> Option<i32>;
    fn close(&mut self);
}

impl<L: Link + ?Sized> Link for Box<L> {
    fn read(&mut self, out: &mut Vec<u8>) -> LinkState {
        (**self).read(out)
    }
    fn write(&mut self, bytes: &[u8]) {
        (**self).write(bytes)
    }
    fn resize(&mut self, cols: u16, rows: u16) {
        (**self).resize(cols, rows)
    }
    fn exit_code(&mut self) -> Option<i32> {
        (**self).exit_code()
    }
    fn close(&mut self) {
        (**self).close()
    }
}

pub struct Terminal<L: Link> {
    link: L,
    pub screen: Screen,
    parser: Parser,
    closed: bool,
    exit: Option<i32>,
    /// How many lines the view is scrolled back (0 = live).
    pub back: usize,
}

impl<L: Link> Terminal<L> {
    pub fn new(link: L, cols: usize, rows: usize) -> Terminal<L> {
        Terminal { link, screen: Screen::new(cols, rows), parser: Parser::new(), closed: false, exit: None, back: 0 }
    }

    /// Read what the program wrote and update the screen. Returns true when anything changed.
    pub fn poll(&mut self) -> bool {
        let before = self.screen.generation;
        let mut buf = Vec::new();
        if self.link.read(&mut buf) == LinkState::Closed {
            self.closed = true;
        }
        if !buf.is_empty() {
            self.parser.feed(&mut self.screen, &buf);
            let reply = self.screen.take_replies();
            if !reply.is_empty() {
                self.link.write(&reply);
            }
            self.back = 0; // output brings the view back to the live screen
        }
        if self.closed && self.exit.is_none() {
            self.exit = self.link.exit_code();
        }
        self.screen.generation != before
    }

    /// `true` once the program has ended and everything it wrote has been read.
    pub fn finished(&mut self) -> bool {
        if self.closed && self.exit.is_none() {
            self.exit = self.link.exit_code();
        }
        self.closed && self.exit.is_some()
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit
    }

    pub fn key(&mut self, k: Key, m: Mods) {
        self.back = 0;
        let bytes = keys::encode(k, m, self.screen.app_cursor_keys);
        if !bytes.is_empty() {
            self.link.write(&bytes);
        }
    }

    pub fn paste(&mut self, text: &str) {
        self.back = 0;
        let bytes = keys::paste(text, self.screen.bracketed_paste);
        self.link.write(&bytes);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.screen.cols() && rows == self.screen.rows() {
            return;
        }
        self.screen.resize(cols, rows);
        self.link.resize(self.screen.cols() as u16, self.screen.rows() as u16);
        self.back = self.back.min(self.screen.scrollback_len());
    }

    pub fn scroll_back(&mut self, lines: isize) {
        let max = self.screen.scrollback_len();
        self.back = (self.back as isize + lines).clamp(0, max as isize) as usize;
    }

    pub fn close(&mut self) {
        self.link.close();
        self.closed = true;
    }

    pub fn link(&mut self) -> &mut L {
        &mut self.link
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Shared {
        incoming: Vec<u8>,
        written: Vec<u8>,
        size: (u16, u16),
        closed: bool,
        exit: Option<i32>,
    }
    struct Fake(Arc<Mutex<Shared>>);
    impl Link for Fake {
        fn read(&mut self, out: &mut Vec<u8>) -> LinkState {
            let mut s = self.0.lock().unwrap();
            out.append(&mut s.incoming);
            if s.closed { LinkState::Closed } else { LinkState::Open }
        }
        fn write(&mut self, b: &[u8]) {
            self.0.lock().unwrap().written.extend_from_slice(b);
        }
        fn resize(&mut self, c: u16, r: u16) {
            self.0.lock().unwrap().size = (c, r);
        }
        fn exit_code(&mut self) -> Option<i32> {
            self.0.lock().unwrap().exit
        }
        fn close(&mut self) {
            self.0.lock().unwrap().closed = true;
        }
    }

    fn term() -> (Terminal<Fake>, Arc<Mutex<Shared>>) {
        let s = Arc::new(Mutex::new(Shared::default()));
        (Terminal::new(Fake(s.clone()), 20, 5), s)
    }

    #[test]
    fn output_reaches_the_screen_keys_reach_the_program_and_answers_go_back() {
        let (mut t, s) = term();
        assert!(!t.poll());
        s.lock().unwrap().incoming.extend_from_slice(b"login: \x1b[6n");
        assert!(t.poll());
        assert_eq!(t.screen.row_text(0), "login:");
        assert_eq!(s.lock().unwrap().written, b"\x1b[1;8R", "the cursor position was asked for and answered");
        s.lock().unwrap().written.clear();
        t.key(Key::Char('a'), Mods::default());
        t.key(Key::Enter, Mods::default());
        t.key(Key::Up, Mods::default());
        t.paste("x\ny");
        assert_eq!(s.lock().unwrap().written, b"a\r\x1b[Ax\ry");
        t.screen.app_cursor_keys = true;
        s.lock().unwrap().written.clear();
        t.key(Key::Up, Mods::default());
        assert_eq!(s.lock().unwrap().written, b"\x1bOA");
    }

    #[test]
    fn resizing_tells_the_program_once_scrollback_is_viewed_and_snaps_back_on_output_or_a_key() {
        let (mut t, s) = term();
        s.lock().unwrap().incoming.extend((0..30).flat_map(|i| format!("line {i}\r\n").into_bytes()));
        t.poll();
        assert!(t.screen.scrollback_len() > 20);
        t.scroll_back(5);
        assert_eq!(t.back, 5);
        t.scroll_back(1000);
        assert_eq!(t.back, t.screen.scrollback_len(), "limited to what exists");
        t.scroll_back(-1000);
        assert_eq!(t.back, 0);
        t.scroll_back(3);
        t.key(Key::Char('q'), Mods::default());
        assert_eq!(t.back, 0, "typing goes back to the live screen");
        t.scroll_back(3);
        s.lock().unwrap().incoming.extend_from_slice(b"new\r\n");
        t.poll();
        assert_eq!(t.back, 0, "so does output");
        t.resize(30, 8);
        assert_eq!(s.lock().unwrap().size, (30, 8));
        s.lock().unwrap().size = (0, 0);
        t.resize(30, 8);
        assert_eq!(s.lock().unwrap().size, (0, 0), "no change, no message to the program");
    }

    #[test]
    fn the_end_of_the_program_is_noticed_after_its_last_output_and_close_hangs_up() {
        let (mut t, s) = term();
        s.lock().unwrap().incoming.extend_from_slice(b"bye\r\n");
        s.lock().unwrap().closed = true;
        t.poll();
        assert!(!t.finished(), "closed but no exit code yet");
        s.lock().unwrap().exit = Some(255);
        assert!(t.finished());
        assert_eq!(t.exit_code(), Some(255));
        assert_eq!(t.screen.row_text(0), "bye");
        let (mut u, s2) = term();
        u.close();
        assert!(s2.lock().unwrap().closed);
    }
}
