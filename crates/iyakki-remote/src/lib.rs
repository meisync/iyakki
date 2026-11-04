//! The shell in a web browser ("pixel streaming"): an HTTPS and WebSocket server of our own that signs people in with the
//! accounts of this machine and streams their screen to a canvas, taking keys and mouse back. Every tile works at once
//! because the browser only ever sees pixels; no tile needs a web version.

pub mod adapt;
pub mod codec;
pub mod devconfig;
pub mod devfw;
pub mod devname;
pub mod devserver;
pub mod http;
pub mod input;
pub mod page;
pub mod relay;
pub mod screen;
pub mod server;
pub mod session;
pub mod tls;
pub mod webswitch;
pub mod ws;

#[cfg(test)]
mod codec_tests;
