//! The web platform, one plug-in at a time. A page's scripts are given only the modules its site is allowed to use;
//! a module that is not plugged in simply does not exist for them (`typeof fetch === "undefined"`).

pub mod bridge;
pub mod console;
pub mod cookies;
pub mod dom;
pub mod encoding;
pub mod esm;
pub mod env;
pub mod fetch;
pub mod idb;
pub mod storage;
pub mod timers;
pub mod url;

pub use bridge::{Bridge, BridgeModule};
pub use console::ConsoleModule;
pub use encoding::EncodingModule;
pub use timers::{run_timers, TimerReport, TimersModule};
pub use url::UrlModule;
pub use cookies::CookiesModule;
pub use esm::EsmModule;
pub use fetch::FetchModule;
pub use idb::{IdbModule, IdbStore};
pub use storage::{KeyValueStore, MemoryStore, StorageModule};
pub use dom::{DomModule, Script};
pub use env::EnvModule;
