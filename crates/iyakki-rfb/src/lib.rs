//! IYAKKI's own RFB (VNC) client, no dependencies.
//!
//! * [`Session`] is the protocol as a state machine without any I/O: bytes from the server go in ([`Session::feed`]), bytes for the
//!   server come out ([`Session::take_output`]), the picture is a [`Framebuffer`] and what happened is a list of [`Event`]s.
//!   Raw, CopyRect, Hextile and ZRLE are decoded (ZRLE with the in-house [`inflate`]); the desktop size and the Extended
//!   Desktop Size resize request are supported. Only security type "None" is accepted: the socket is a private Unix socket.
//! * [`Link`] runs a session on a Unix socket in a thread.
//!
//! Everything the server sends is untrusted: sizes are bounded ([`MAX_DIM`], [`MAX_PIXELS`], [`MAX_CUT`], ...), a rectangle outside
//! the picture, an unknown encoding or a bad stream is an [`Error`] (the session is then closed), and nothing panics.

pub mod inflate;
pub mod keysyms;
mod link;
mod session;

pub use link::Link;
pub use session::{Error, Event, Framebuffer, Rect, Session};

/// Largest width or height of the remote screen.
pub const MAX_DIM: u32 = 8192;
/// Largest number of pixels of the remote screen (4096 x 4096: 64 MiB of picture).
pub const MAX_PIXELS: u64 = 4096 * 4096;
/// Largest clipboard text taken from or sent to the server (bytes).
pub const MAX_CUT: usize = 1 << 20;
/// Largest ZRLE rectangle payload (compressed bytes).
pub const MAX_ZRLE: usize = 64 << 20;
