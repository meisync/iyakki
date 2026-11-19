//! The scene drawing (moved to `iyakki-shared` for F-SPLIT-03); the agent keeps the external-agent chat backend and the tests.

pub use iyakki_shared::scene::*;
pub mod agentchat;
#[cfg(test)]
mod tests;
