//! A terminal in plain Rust, no dependencies but `libc`: an escape-sequence parser (VT100 / ECMA-48 and the xterm parts that `top`,
//! `vi`, `less`, `htop`, `tmux` and `ssh` sessions use), a screen with scrollback, the alternate screen and colours, the keys a
//! program expects, and, on Linux and macOS, a pseudo-terminal that starts a program on it (F-CLI-08, docs/CLI-SSH.md).
//!
//! Not done: mouse reporting, sixel and other pictures, right-to-left text, and combining marks (they are dropped).

pub mod keys;
mod parser;
#[cfg(unix)]
pub mod pty;
mod screen;
pub mod terminal;
#[cfg(unix)]
pub mod session;

pub use keys::{Key, Mods};
pub use terminal::{Link, LinkState, Terminal};
pub use parser::Parser;
pub use screen::{char_width, Cell, Color, Pen, Screen};
pub use screen::{BLINK, BOLD, DIM, HIDDEN, ITALIC, REVERSE, STRIKE, UNDERLINE};

#[cfg(test)]
mod tests;
#[cfg(all(test, target_os = "linux"))]
mod real_ssh_tests;

#[cfg(test)]
mod fuzzkit;
#[cfg(test)]
mod fuzz_tests;
