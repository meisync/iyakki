//! Host side of the tile ABI (docs/development/TILE-ABI.md).
//!
//! * [`TileRunner`] loads a tile through the `iyakki-wasm` sandbox and drives it.
//! * [`render`] is a reference software renderer (tests assert pixels).
//! * [`export`] turns a display list into JSON or SVG for a browser.
//!
//! Nothing here reads /proc, /sys or /dev; the tile sees only what the imports give it.
pub mod export;
pub mod render;
mod runner;

pub use runner::{Budgets, ServiceBackend, TileError, TileManifest, TileRunner, FrameStats};

#[cfg(test)]
mod tests;
