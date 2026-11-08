//! The tile ABI (docs/development/TILE-ABI.md): the types a WASM tile and its host share.
//!
//! A tile receives [`Event`]s and returns a [`DisplayList`]. Both have a compact binary encoding
//! (LEB128 varints, zigzag for signed values, colours as 4 bytes) written by hand here so the same
//! few hundred lines serve the guest (no_std), the device host and a browser host. Decoding hostile
//! or truncated bytes never panics and never allocates more than the limits below allow.
#![no_std]
extern crate alloc;
#[cfg(test)]
extern crate std;

mod codec;
mod display;
mod event;
mod metrics;

pub use codec::DecodeError;
pub use display::{Color, DisplayList, Op, Rect};
pub use event::{Event, EventBatch};
pub use metrics::{line_height, measure_text};

/// The ABI version a host and a tile must agree on exactly. A tile that reports another number is
/// refused at load time (see TILE-ABI.md, "Versioning").
pub const ABI_VERSION: u32 = 1;

/// Magic at the start of every encoded display list.
pub const LIST_MAGIC: [u8; 4] = *b"IYDL";
/// Magic at the start of every encoded event batch.
pub const EVENT_MAGIC: [u8; 4] = *b"IYEV";

/// Hard limits. Encoders refuse to exceed them and decoders reject anything larger.
pub mod limits {
    /// Operations in one display list.
    pub const MAX_OPS: usize = 8192;
    /// Encoded size of one display list, in bytes.
    pub const MAX_LIST_BYTES: usize = 256 * 1024;
    /// Bytes of one text run or one text-input event.
    pub const MAX_TEXT: usize = 1024;
    /// Absolute value of any coordinate or size, in logical pixels.
    pub const MAX_COORD: i32 = 32768;
    /// Nesting of clip pushes.
    pub const MAX_CLIP_DEPTH: usize = 16;
    /// Events in one batch.
    pub const MAX_EVENTS: usize = 256;
    /// Encoded size of one event batch, in bytes.
    pub const MAX_EVENT_BYTES: usize = 64 * 1024;
    /// Largest text size (logical pixels) a run may ask for.
    pub const MAX_FONT_SIZE: u32 = 512;
}

#[cfg(test)]
mod tests;
