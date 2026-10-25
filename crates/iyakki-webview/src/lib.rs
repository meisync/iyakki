//! The embedded full web engine, shell side. See `docs/WEBVIEW-PROTOCOL.md` for the wire format.
//!
//! The engine (WPE WebKit on the image, WebKitGTK on a development desktop) runs as a separate *helper program* that renders
//! offscreen. The shell and the helper talk over a Unix socket (framed messages, [`proto`]) and the pixels travel through a
//! shared file ([`client`]). Nothing here links a web engine, and nothing here has dependencies.

pub mod blit;
pub mod client;
pub mod keys;
pub mod proto;

pub use client::{Config, Event, WebView};
pub use proto::{FromHelper, Hello, MouseKind, Rect, ToHelper};
