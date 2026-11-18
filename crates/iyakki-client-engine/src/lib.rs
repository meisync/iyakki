//! The IYAKKI client engine, picture mode (docs/development/CLIENT-SPLIT-DESIGN.md, step 4). EXPERIMENTAL.
//!
//! * [`engine`]: the state machine of the client session protocol (`iyakki_core_api::client`), no sockets, no clock.
//! * [`picture`]: the tile decoder (a wasm-safe copy of `iyakki_link::pixelcodec`'s, tested to agree).
//! * [`sign`]: the `IYAKKI-SIG` answer, and the fallback Ed25519 key.
//! * [`uidraw`]: the UI SDK's display list drawn into a framebuffer (shapes; text runs go to the page), the accessibility tree and the input lines.
//! * [`host`]: the JSON command layer; `ffi` (wasm32 only) is the plain `extern "C"` surface the page's glue calls.
//!
//! No policy lives here: the instance decides what a client may do. The engine holds no data and no devices.
#![deny(unsafe_code)]

pub mod engine;
pub mod host;
pub mod picture;
pub mod sign;
pub mod uidraw;

#[cfg(target_arch = "wasm32")]
mod ffi;

// the assets are for native hosts (the server that serves the page); the wasm module must not contain its own earlier build
#[cfg(not(target_arch = "wasm32"))]
/// The hand-written glue that loads the module and drives it from a page (browser); the window program reuses it in its web view.
pub const GLUE_JS: &str = include_str!("../js/engine.js");
#[cfg(not(target_arch = "wasm32"))]
/// The browser key store (WebCrypto Ed25519 with an IndexedDB fallback).
pub const KEYS_JS: &str = include_str!("../js/keys.js");
#[cfg(not(target_arch = "wasm32"))]
/// The media half of the page: the machine's sound, the clipboard on a button press, a machine camera, this device's camera and microphone.
pub const MEDIA_JS: &str = include_str!("../js/media.js");
#[cfg(not(target_arch = "wasm32"))]
/// The audio worklet (player and microphone capture); also holds the queue logic the node tests exercise.
pub const WORKLET_JS: &str = include_str!("../js/audio-worklet.js");
#[cfg(not(target_arch = "wasm32"))]
/// The built module (`dist/client-engine/build.sh`), committed; empty if the build script has never been run.
pub const WASM: &[u8] = include_bytes!("../assets/iyakki-client-engine.wasm");
