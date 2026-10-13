//! `iyakki-usb`: write an IYAKKI image to a USB stick without typing `dd` and without risking your system disk.
//!
//! * [`drives`]: find the disks (Linux via `/sys`, macOS via `diskutil`) and decide which ones may be written.
//! * [`writer`]: write the image, verify it by reading it back, with progress.
//! * [`ui`]: the arrow-key menu, the erase warning and the progress bar, as plain strings (testable).
//! * [`term`]: the raw-terminal glue (the only place that talks to the terminal driver).
//! * [`macos`]: parsers for `diskutil` output, plus the calls to it.
//!
//! This is a tool for the user's own computer, not part of the IYAKKI image (see BOUNDARY.md).

pub mod cli;
pub mod drives;
pub mod macos;
pub mod term;
pub mod ui;
pub mod writer;
