//! An in-house PDF reader and writer for untrusted files (docs/PDF.md).
//!
//! Reading: file structure (cross-reference tables and streams, object streams, incremental updates, repair by scanning), the page
//! tree, text extraction with positions and search, and a simple page renderer to an RGBA bitmap. Writing: a small multi-page writer
//! for text, headings, lists and pictures. Hostile files are expected: every size, count and depth is capped and nothing panics.
//!
//! Not a full PDF engine. Not supported: encryption (a password-protected file is refused with a clear message), editing forms,
//! signatures, JPEG 2000, JBIG2 and CCITT pictures, patterns, shadings, soft masks, Type 3 glyph drawing, embedded font programs
//! (a substitute font draws the text), optional content layers.

pub mod color;
pub mod doc;
pub mod docconv;
pub mod file;
pub mod filter;
pub mod font;
pub mod geom;
pub mod image;
pub mod interp;
pub mod obj;
pub mod raster;
pub mod render;
pub mod text;
pub mod write;

pub use doc::{Document, FormField, Info, Rendered};
pub use file::PdfError;
pub use render::{BoxGlyphs, FontStyle, GlyphBitmap, GlyphSource};
pub use text::Hit;
pub use write::{Block, Builder, Built, PageSize, PdfImage};

#[cfg(test)]
mod tests_support;
#[cfg(test)]
mod tests;
