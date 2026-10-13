//! The editing engine behind IYAKKI's EDIT tile and the chat's "edit with assistant" mode.
//!
//! Everything here is plain Rust with no dependencies and no I/O, so it is easy to test and audit:
//!
//! * [`buffer`]: text buffer with cursor, selection, undo/redo, search and replace.
//! * [`syntax`]: small hand-written highlighter (Rust, Python, JS/TS, C/C++, Java, Go, shell, JSON,
//!   TOML, YAML, Markdown) that works line by line with a tiny carried state.
//! * [`diff`]: Myers line diff, grouped into hunks that can be accepted or rejected one by one.
//! * [`proposal`]: turns an assistant's reply (SEARCH/REPLACE blocks or a whole-file block) into new
//!   file text, refusing anything ambiguous.

pub mod buffer;
pub mod diff;
pub mod proposal;
pub mod syntax;

pub use buffer::{Buffer, EditError, Move, Pos};
pub use diff::{apply_hunks, diff_hunks, Hunk};
pub use syntax::{highlight_line, lang_for_path, HlState, Highlighter, Lang, Span, TokenKind};
