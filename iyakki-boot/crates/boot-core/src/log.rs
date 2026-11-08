// SPDX-License-Identifier: MIT OR Apache-2.0
//! The boot log kept in memory so the loader can write it to a file on the boot volume. A laptop has no serial port and the
//! splash hides the text console, so without this file a start that stops leaves no trace. The buffer keeps the first `cap`
//! bytes (the start of a run says what was found and what was chosen) and marks once that later text was dropped.

use alloc::string::String;
use alloc::vec::Vec;

/// What is appended once when the cap is reached.
pub const CUT_MARK: &str = "\n[log cut: the cap was reached]\n";

/// A capped, append-only text buffer.
pub struct LogBuffer {
    buf: Vec<u8>,
    cap: usize,
    cut: bool,
}

impl LogBuffer {
    /// An empty buffer that keeps at most `cap` bytes of log text (the cut mark may follow them).
    pub const fn new(cap: usize) -> LogBuffer {
        LogBuffer { buf: Vec::new(), cap, cut: false }
    }

    /// Append text. Text past the cap is dropped, and the cut mark is added once.
    pub fn push_str(&mut self, s: &str) {
        if self.cut {
            return;
        }
        let room = self.cap.saturating_sub(self.buf.len());
        if s.len() <= room {
            self.buf.extend_from_slice(s.as_bytes());
            return;
        }
        // keep whole characters only
        let mut n = room;
        while n > 0 && !s.is_char_boundary(n) {
            n -= 1;
        }
        self.buf.extend_from_slice(&s.as_bytes()[..n]);
        self.buf.extend_from_slice(CUT_MARK.as_bytes());
        self.cut = true;
    }

    /// The text so far.
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// The text so far as a string (for tests and messages).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }

    /// Nothing was logged yet.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_kept_in_order_until_the_cap_then_cut_once() {
        let mut l = LogBuffer::new(32);
        l.push_str("boot: started\n");
        l.push_str("item: 0 iyakki\n");
        assert_eq!(l.text(), "boot: started\nitem: 0 iyakki\n");
        l.push_str("start: iyakki kernel=/boot/bzImage\n");
        let t = l.text();
        assert!(t.starts_with("boot: started\nitem: 0 iyakki\nst"), "{t}");
        assert!(t.ends_with(CUT_MARK), "{t}");
        assert_eq!(t.len(), 32 + CUT_MARK.len());
        l.push_str("boot: more\n");
        assert_eq!(l.text().len(), 32 + CUT_MARK.len(), "nothing is added after the cut");
    }

    #[test]
    fn a_multibyte_character_is_never_split_at_the_cap() {
        let mut l = LogBuffer::new(5);
        l.push_str("ab€cd"); // € is three bytes: 2 + 3 = 5 fits exactly, 'c' does not
        assert_eq!(l.text(), alloc::format!("ab€{CUT_MARK}"));
        let mut l = LogBuffer::new(4);
        l.push_str("ab€cd"); // only 4 bytes of room: the € must be dropped whole
        assert_eq!(l.text(), alloc::format!("ab{CUT_MARK}"));
    }

    #[test]
    fn an_empty_buffer_says_so() {
        let l = LogBuffer::new(8);
        assert!(l.is_empty());
        assert!(l.bytes().is_empty());
    }
}
