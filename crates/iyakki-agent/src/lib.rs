//! Agent-facing building blocks behind the shell's apps: CHAT (LLM client), RAG (local
//! retrieval), MCP (Model Context Protocol client) and CLI (policy-checked command line).
//! Everything is headless and unit-tested; `iyakki-shell` only draws it.

pub use iyakki_proxy;

pub mod agent;

pub mod chat;
pub mod cli;
pub mod embed;
pub mod kb;
pub mod config;
pub mod context;
pub mod mcp;
pub mod providers;
pub mod vault;
pub mod rag;
pub mod rag_ext;
pub mod services;
pub mod sessions;
pub mod scope;
pub mod runs;
pub mod settings;
pub mod consul;
pub mod pkg;
pub mod wtool;
pub mod media;
pub mod mediaindex;
pub mod secretreq;
pub mod mediadl;
pub mod mediaqueue;
pub mod mediaserve;
pub mod credguard;
pub mod radio;
pub mod radio_stations;
pub mod radiotools;
pub mod webtools;
pub mod skills;
pub mod voice;
pub mod webengine;
pub mod exec;
pub mod clock;
pub mod regex;
pub mod textcmd;
pub mod toollog;
pub mod usage;
pub mod models;
pub mod calendar;
pub mod archive;
pub mod clis;
pub mod cluster;
pub mod ollama;
pub mod devwatch;
pub mod release;
pub mod sysinfo;
pub mod inbox;

use iyakki_sec::policy::Policy;

/// What the apps need from the running system. Implemented by `iyakki-core` (and by mocks in tests)
/// so this crate does not depend on the supervisor.
pub trait HostApi: Send + Sync {
    fn detect_env(&self) -> String;
    fn net_status(&self) -> String;
    /// Run a WASM module for `user` with that user's limits and capability tokens.
    fn run_wasm(&self, user: &str, policy: &Policy, wasm: &[u8], entry: &str) -> Result<String, String>;
    fn power_off(&self);
    fn reboot(&self);
}
