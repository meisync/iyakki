//! An in-house PDF reader and writer for untrusted files (docs/PDF.md).
//!
//! Reading: file structure (cross-reference tables and streams, object streams, incremental updates, repair by scanning), the page
//! tree, text extraction with positions and search, and a simple page renderer to an RGBA bitmap. Writing: a small multi-page writer
//! for text, headings, lists and pictures. Hostile files are expected: every size, count and depth is capped and nothing panics.
//!
//! Not a full PDF engine. Passwords: files protected with RC4 (40 to 128 bit) or AES-128 open with the password ([`file::File::open_with_password`]); AES-256 is refused with a clear message. Forms: text, choice and check box fields can be filled in and written as an incremental update ([`form`]). Not supported: editing the other kinds of fields,
//! signatures, JPEG 2000, JBIG2 and CCITT pictures, patterns, shadings, soft masks, Type 3 glyph drawing, embedded font programs
//! (a substitute font draws the text), optional content layers.

pub mod color;
pub mod crypt;
pub mod doc;
pub mod embed;
pub mod docconv;
pub mod file;
pub mod filter;
pub mod form;
pub mod font;
pub mod geom;
pub mod image;
pub mod interp;
pub mod md;
pub mod obj;
pub mod order;
pub mod raster;
pub mod render;
pub mod text;
pub mod ttf;
pub mod write;

pub use doc::{Document, FormField, Info, Rendered};
pub use form::{EditableField, FieldKind};
pub use file::PdfError;
pub use render::{BoxGlyphs, FontStyle, GlyphBitmap, GlyphSource};
pub use text::{CharBox, Hit};
pub use write::{Block, Builder, Built, PageSize, PdfImage};

#[cfg(test)]
mod tests_support;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod form_tests;
#[cfg(test)]
mod md_tests;
#[cfg(test)]
mod embed_tests;
#[cfg(test)]
mod real_reader_tests;
#[cfg(test)]
mod real_gs_tests;

#[cfg(test)]
mod fuzzkit;
#[cfg(test)]
mod fuzz_tests;
