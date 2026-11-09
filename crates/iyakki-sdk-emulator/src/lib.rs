//! The SDK emulator (F-SDK-02, docs/SDK-EMULATOR.md): run a WASM tile on the developer's own computer against fake bridge services,
//! without a device.
//!
//! * [`project`]: the tile folder, its manifest, the install-card review text.
//! * [`services`]: the fake bridge (clock, settings, storage, scratch files, vault, canned network, notifications, audio and camera stubs)
//!   with the call log, the prompts and the manifest violations.
//! * [`emulator`]: the tile in the real sandbox, driven by inputs; reload with replay.
//! * [`script`], [`golden`], [`png`]: input scripts and golden-picture regression tests (`iyakki sdk test`).
//! * [`watch`]: change detection for hot reload. [`cli`]: the commands.
//!
//! Headless and dependency-light: the window is a separate binary (crates/iyakki-gui/src/sdk_emulate.rs) that drives [`emulator::Emulator`].

pub mod cli;
pub mod emulator;
pub mod golden;
pub mod png;
pub mod project;
pub mod script;
pub mod services;
pub mod sha256;
pub mod watch;

#[cfg(test)]
mod tests;
