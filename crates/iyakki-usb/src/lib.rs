//! `iyakki-usb`: write an IYAKKI image to a USB stick without typing `dd` and without risking your system disk.
//!
//! * [`drives`]: find the disks (Linux via `/sys`, macOS via `diskutil`) and decide which ones may be written.
//! * [`install`], [`gpt_install`], [`fatclone`]: the installer (MBR or GPT layout, 512 or 4096-byte sectors).
//! * [`writer`]: write the image, verify it by reading it back, with progress.
//! * [`ui`]: the arrow-key menu, the erase warning and the progress bar, as plain strings (testable).
//! * [`term`]: the raw-terminal glue (the only place that talks to the terminal driver).
//! * [`macos`]: parsers for `diskutil` output, plus the calls to it.
//!
//! This is a tool for the user's own computer, not part of the IYAKKI image (see BOUNDARY.md).

pub mod apply;
pub mod biosgpt;
pub mod biosshared;
pub mod bootpack;
pub mod cli;
pub mod drives;
pub mod efi;
pub mod elevate;
pub mod fetch;
pub mod fatclone;
pub mod fatwrite;
pub mod imagearch;
pub mod gpt_install;
pub mod install;
pub mod install_cli;
pub mod json;
pub mod leanstrip;
pub mod plan_cli;
pub mod planner;
pub mod resize;
pub mod sbenroll;
pub mod sbenroll_cli;
pub mod sbenroll_write;
pub mod resize_step;
pub mod screen;
pub mod stickupdate;
pub mod macos;
pub mod term;
pub mod tui;
pub mod ui;
pub mod windows;
#[cfg(windows)]
pub mod windows_real;
pub mod wizard;
pub mod writer;
pub mod writerwin;
pub mod writerwin_run;

#[cfg(test)]
mod biosgpt_tests;
#[cfg(test)]
mod gpt_install_tests;
#[cfg(test)]
mod planner_tests;
#[cfg(test)]
mod real_mkfs_tests;
#[cfg(test)]
mod resize_tests;
#[cfg(test)]
mod resize_step_tests;
#[cfg(test)]
mod sbenroll_tests;

#[cfg(test)]
mod fuzzkit;
#[cfg(test)]
mod bios_fuzz_tests;
