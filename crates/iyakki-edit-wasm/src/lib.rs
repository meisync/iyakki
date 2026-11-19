//! `iyakki-edit` compiled to WebAssembly for the browser editor (F-WEB-05). The module needs no imports: it keeps its buffers in its
//! own memory and answers through one result area the page reads after each call. The interface is C-style (numbers and byte
//! ranges of the module's memory) so a page needs no glue library:
//!
//! * memory: `ed_alloc(len)` gives a block to write text into, `ed_dealloc(ptr, len)` takes it back;
//! * a buffer is a number (0 is never a buffer): `ed_new(ptr, len)` (0 when the text is too large or binary), `ed_free(h)`;
//! * editing: `ed_cursor(h, line, col)`, `ed_insert(h, ptr, len)`, `ed_newline(h)`, `ed_backspace(h)`, `ed_delete(h)`, `ed_undo(h)`,
//!   `ed_redo(h)` return 0 when done and 1 when refused (nothing to undo, a limit reached);
//! * reading: `ed_text(h)`, `ed_find_all(h, ptr, len, case_sensitive)` (JSON `[[l1,c1,l2,c2],...]`), `ed_highlight(h, lang_ptr, lang_len,
//!   line)` (JSON `[[start,end,"kind"],...]`) and `ed_diff(ptr_a, len_a, ptr_b, len_b)` (JSON hunks) put their answer in the result
//!   area and return its length; `ed_result()` is where it is, valid until the next call.
//!
//! The calls are `unsafe extern "C"` only because they take raw addresses of the module's own memory; each checks lengths and never
//! reads outside the block it was given.
// SPDX-License-Identifier: Apache-2.0

use iyakki_edit::{buffer::Buffer, diff, syntax};
use std::cell::RefCell;
use std::collections::BTreeMap;

struct World {
    next: u32,
    buffers: BTreeMap<u32, Buffer>,
    result: Vec<u8>,
}

thread_local! {
    static W: RefCell<World> = const { RefCell::new(World { next: 1, buffers: BTreeMap::new(), result: Vec::new() }) };
}

/// At most this many buffers at once (a page opens a few files, not thousands).
pub const MAX_BUFFERS: usize = 64;

fn with<R>(f: impl FnOnce(&mut World) -> R) -> R {
    W.with(|w| f(&mut w.borrow_mut()))
}

/// The text at `ptr` as a string (invalid UTF-8 becomes `None`).
///
/// # Safety
/// `ptr` and `len` must describe a block of this module's memory (what `ed_alloc` gave, or a page string copied into it).
unsafe fn text_at(ptr: *const u8, len: usize) -> Option<String> {
    if len == 0 {
        return Some(String::new());
    }
    if ptr.is_null() || len > iyakki_edit::buffer::MAX_BYTES {
        return None;
    }
    // SAFETY: the caller promises the block; the length is bounded above
    String::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()).ok()
}

fn put(w: &mut World, s: String) -> u32 {
    w.result = s.into_bytes();
    w.result.len() as u32
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

#[no_mangle]
pub extern "C" fn ed_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr` must come from `ed_alloc(len)` with the same `len`, and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn ed_dealloc(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        // SAFETY: the caller promises it is a block of `ed_alloc(len)`
        drop(unsafe { Vec::from_raw_parts(ptr, 0, len.max(1)) });
    }
}

/// Where the last answer is (`ed_result_len` bytes).
#[no_mangle]
pub extern "C" fn ed_result() -> *const u8 {
    with(|w| w.result.as_ptr())
}

/// # Safety
/// See [`text_at`].
#[no_mangle]
pub unsafe extern "C" fn ed_new(ptr: *const u8, len: usize) -> u32 {
    // SAFETY: forwarded promise
    let Some(t) = (unsafe { text_at(ptr, len) }) else { return 0 };
    with(|w| {
        if w.buffers.len() >= MAX_BUFFERS {
            return 0;
        }
        let Ok(b) = Buffer::from_text(&t) else { return 0 };
        let h = w.next;
        w.next += 1;
        w.buffers.insert(h, b);
        h
    })
}

#[no_mangle]
pub extern "C" fn ed_free(h: u32) {
    with(|w| w.buffers.remove(&h));
}

fn edit(h: u32, f: impl FnOnce(&mut Buffer) -> bool) -> i32 {
    with(|w| match w.buffers.get_mut(&h) {
        Some(b) => i32::from(!f(b)),
        None => 1,
    })
}

#[no_mangle]
pub extern "C" fn ed_cursor(h: u32, line: usize, col: usize) -> i32 {
    edit(h, |b| {
        b.set_cursor((line.min(b.line_count().saturating_sub(1)), col), false);
        true
    })
}

/// # Safety
/// See [`text_at`].
#[no_mangle]
pub unsafe extern "C" fn ed_insert(h: u32, ptr: *const u8, len: usize) -> i32 {
    // SAFETY: forwarded promise
    let Some(t) = (unsafe { text_at(ptr, len) }) else { return 1 };
    edit(h, |b| b.insert(&t).is_ok())
}

#[no_mangle]
pub extern "C" fn ed_newline(h: u32) -> i32 {
    edit(h, |b| b.newline().is_ok())
}

#[no_mangle]
pub extern "C" fn ed_backspace(h: u32) -> i32 {
    edit(h, |b| b.backspace().is_ok())
}

#[no_mangle]
pub extern "C" fn ed_delete(h: u32) -> i32 {
    edit(h, |b| b.delete().is_ok())
}

#[no_mangle]
pub extern "C" fn ed_undo(h: u32) -> i32 {
    edit(h, |b| b.undo())
}

#[no_mangle]
pub extern "C" fn ed_redo(h: u32) -> i32 {
    edit(h, |b| b.redo())
}

/// The whole text; 0 length also for an unknown buffer (check `ed_has(h)`).
#[no_mangle]
pub extern "C" fn ed_text(h: u32) -> u32 {
    with(|w| match w.buffers.get(&h).map(|b| b.text()) {
        Some(t) => put(w, t),
        None => put(w, String::new()),
    })
}

#[no_mangle]
pub extern "C" fn ed_has(h: u32) -> i32 {
    with(|w| i32::from(w.buffers.contains_key(&h)))
}

#[no_mangle]
pub extern "C" fn ed_dirty(h: u32) -> i32 {
    with(|w| w.buffers.get(&h).map_or(0, |b| i32::from(b.is_dirty())))
}

/// # Safety
/// See [`text_at`].
#[no_mangle]
pub unsafe extern "C" fn ed_find_all(h: u32, ptr: *const u8, len: usize, case_sensitive: i32) -> u32 {
    // SAFETY: forwarded promise
    let q = unsafe { text_at(ptr, len) }.unwrap_or_default();
    with(|w| {
        let hits = match w.buffers.get(&h) {
            Some(b) if !q.is_empty() => b.find_all(&q, case_sensitive != 0),
            _ => vec![],
        };
        let body: Vec<String> = hits.iter().take(5000).map(|((a, b), (c, d))| format!("[{a},{b},{c},{d}]")).collect();
        put(w, format!("[{}]", body.join(",")))
    })
}

/// # Safety
/// See [`text_at`].
#[no_mangle]
pub unsafe extern "C" fn ed_highlight(h: u32, lang_ptr: *const u8, lang_len: usize, line: usize) -> u32 {
    // SAFETY: forwarded promise
    let name = unsafe { text_at(lang_ptr, lang_len) }.unwrap_or_default();
    with(|w| {
        let lang = syntax::lang_for_path(&name);
        let spans = match w.buffers.get(&h) {
            Some(b) if line < b.line_count() => syntax::Highlighter::new(lang).line_spans(b.lines(), line),
            _ => vec![],
        };
        let body: Vec<String> = spans.iter().map(|s| format!("[{},{},\"{:?}\"]", s.start, s.end, s.kind)).collect();
        put(w, format!("[{}]", body.join(",")))
    })
}

/// # Safety
/// See [`text_at`].
#[no_mangle]
pub unsafe extern "C" fn ed_diff(a_ptr: *const u8, a_len: usize, b_ptr: *const u8, b_len: usize) -> u32 {
    // SAFETY: forwarded promise
    let (Some(a), Some(b)) = (unsafe { text_at(a_ptr, a_len) }, unsafe { text_at(b_ptr, b_len) }) else { return with(|w| put(w, "null".into())) };
    let (old, new): (Vec<String>, Vec<String>) = (a.lines().map(str::to_string).collect(), b.lines().map(str::to_string).collect());
    with(|w| {
        let hunks = diff::diff_hunks(&old, &new);
        let one = |h: &diff::Hunk| {
            let list = |v: &[String]| v.iter().map(|s| format!("\"{}\"", esc(s))).collect::<Vec<_>>().join(",");
            format!("{{\"old_start\":{},\"old\":[{}],\"new\":[{}]}}", h.old_start, list(&h.old), list(&h.new))
        };
        put(w, format!("[{}]", hunks.iter().map(one).collect::<Vec<_>>().join(",")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> (*const u8, usize) {
        (text.as_ptr(), text.len())
    }

    fn result() -> String {
        let n = with(|w| w.result.len());
        // SAFETY: the result area is `n` bytes long and nothing else runs on this thread
        String::from_utf8(unsafe { std::slice::from_raw_parts(ed_result(), n) }.to_vec()).unwrap()
    }

    fn text_of(h: u32) -> String {
        ed_text(h);
        result()
    }

    #[test]
    fn a_buffer_is_edited_undone_and_read_back_through_the_interface() {
        let (p, n) = s("fn main() {}\nlet x = 1;\n");
        // SAFETY: the block is the test's own string
        let h = unsafe { ed_new(p, n) };
        assert!(h > 0 && ed_has(h) == 1);
        assert_eq!(text_of(h), "fn main() {}\nlet x = 1;\n");
        assert_eq!(ed_cursor(h, 1, 3), 0);
        let (p, n) = s("y");
        assert_eq!(unsafe { ed_insert(h, p, n) }, 0);
        assert_eq!(text_of(h), "fn main() {}\nlety x = 1;\n");
        assert_eq!(ed_dirty(h), 1);
        assert_eq!(ed_backspace(h), 0);
        assert_eq!(ed_newline(h), 0);
        assert_eq!(text_of(h), "fn main() {}\nlet\n x = 1;\n".replace("let\n x", "let\n x"), "the newline splits the line at the cursor");
        assert_eq!(ed_undo(h), 0);
        assert_eq!(ed_undo(h), 0);
        assert_eq!(ed_undo(h), 0);
        assert_eq!(text_of(h), "fn main() {}\nlet x = 1;\n");
        assert_eq!(ed_undo(h), 1, "nothing left to undo is a refusal, not a crash");
        assert_eq!(ed_redo(h), 0);
        ed_free(h);
        assert_eq!(ed_has(h), 0);
        assert_eq!(ed_insert_str(h, "z"), 1, "a freed buffer refuses");
    }

    fn ed_insert_str(h: u32, t: &str) -> i32 {
        let (p, n) = s(t);
        // SAFETY: the test's own string
        unsafe { ed_insert(h, p, n) }
    }

    #[test]
    fn search_highlighting_and_diff_answer_in_json() {
        let (p, n) = s("let a = \"x\"; // note\nlet b = 2;\n");
        let h = unsafe { ed_new(p, n) };
        let (q, qn) = s("LET");
        unsafe { ed_find_all(h, q, qn, 0) };
        assert_eq!(result(), "[[0,0,0,3],[1,0,1,3]]");
        unsafe { ed_find_all(h, q, qn, 1) };
        assert_eq!(result(), "[]", "case sensitive finds none");
        let (l, ln) = s("main.rs");
        unsafe { ed_highlight(h, l, ln, 0) };
        let spans = result();
        assert!(spans.starts_with("[[0,3,\"Keyword\"]") && spans.contains("\"String\"") && spans.contains("\"Comment\""), "{spans}");
        unsafe { ed_highlight(h, l, ln, 99) };
        assert_eq!(result(), "[]", "a line past the end has no spans");
        let (a, an) = s("one\ntwo\nthree\n");
        let (b, bn) = s("one\n2 \"quoted\"\nthree\nfour\n");
        unsafe { ed_diff(a, an, b, bn) };
        let d = result();
        assert_eq!(d, "[{\"old_start\":1,\"old\":[\"two\"],\"new\":[\"2 \\\"quoted\\\"\"]},{\"old_start\":3,\"old\":[],\"new\":[\"four\"]}]");
        ed_free(h);
    }

    #[test]
    fn bad_input_is_refused_and_the_number_of_buffers_is_bounded() {
        assert_eq!(unsafe { ed_new(std::ptr::null(), 5) }, 0, "no address");
        let bad = [0xffu8, 0xfe, 0x00];
        assert_eq!(unsafe { ed_new(bad.as_ptr(), bad.len()) }, 0, "not text");
        assert_eq!(unsafe { ed_new(bad[..0].as_ptr(), 0) } > 0, true, "an empty file is a buffer");
        let mut held = Vec::new();
        for _ in 0..MAX_BUFFERS + 4 {
            let h = unsafe { ed_new(b"x".as_ptr(), 1) };
            if h != 0 {
                held.push(h);
            }
        }
        assert!(held.len() < MAX_BUFFERS + 4 && with(|w| w.buffers.len()) <= MAX_BUFFERS);
        for h in held {
            ed_free(h);
        }
        // the result area of an unknown buffer is empty, a diff of non-text is `null`
        assert_eq!(ed_text(9999), 0);
        unsafe { ed_diff(bad.as_ptr(), bad.len(), b"a".as_ptr(), 1) };
        assert_eq!(result(), "null");
        // alloc and dealloc pair up
        let p = ed_alloc(100);
        unsafe { ed_dealloc(p, 100) };
    }
}
