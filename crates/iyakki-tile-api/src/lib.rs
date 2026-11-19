//! The facade between the shell and a tile crate (docs/development/COMPILE-SPLIT-PILOT.md, "Second step").
//!
//! A tile crate implements [`App`] and gets a [`TileCx`] on every call. `TileCx` holds a few plain fields and trait objects, only for what
//! the extracted tiles use; the shell fills it from its own `AppCx` and wraps each tile crate's `App` in an adapter for the shell's own
//! `App` trait. Tests of a tile crate fill it with fakes. Add a field or a trait here when the next tile needs it, not before.

pub mod calendar;
pub mod cameras;
pub mod pdf;
pub mod radio;
pub mod remote;
pub mod usage;

#[cfg(feature = "ui")]
mod ui;
#[cfg(feature = "ui")]
pub use ui::*;
