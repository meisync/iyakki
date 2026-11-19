//! The web platform, one plug-in at a time. A page's scripts are given only the modules its site is allowed to use;
//! a module that is not plugged in simply does not exist for them (`typeof fetch === "undefined"`).

pub mod boxes;
pub mod bridge;
pub mod console;
pub mod cookies;
pub mod dom;
pub mod dom_layout;
pub mod encoding;
pub mod esm;
pub mod env;
pub mod fetch;
pub mod glyphs;
pub mod idb;
pub mod passkeys;
pub mod storage;
pub mod timers;
pub mod url;
pub mod webfont;

pub use bridge::{Bridge, BridgeModule};
pub use console::ConsoleModule;
pub use encoding::EncodingModule;
pub use timers::{run_timers, TimerReport, TimersModule};
pub use url::UrlModule;
pub use cookies::CookiesModule;
pub use esm::EsmModule;
pub use fetch::FetchModule;
pub use idb::{IdbModule, IdbStore};
pub use passkeys::{PasskeyHost, PasskeysModule};
pub use storage::{KeyValueStore, MemoryStore, StorageModule};
#[cfg(test)]
mod dom_extra_tests;

pub use dom::{DomModule, Script};
pub use dom_layout::LayoutEnv;
pub use env::EnvModule;
#[cfg(test)]
mod fuzzkit;
#[cfg(test)]
mod fuzz_layout_tests;
#[cfg(test)]
mod boxes_chrome_tests;
#[cfg(test)]
mod styles_chrome_tests;
#[cfg(test)]
mod dom_chrome_tests;
