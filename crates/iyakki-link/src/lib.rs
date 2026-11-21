//! The small crate behind the desktop connector (docs/development/CONNECTOR-APP-PLAN.md, section 1): the client, the WebSocket framing,
//! the picture decoder and the certificate reader. The agent re-exports these; `iyakki-connect` uses them without the agent.

pub mod certinfo;
pub mod cloudcarrier;
pub mod cloudclient;
pub mod cloudtiles;
pub mod pixelcodec;
pub mod rawlink;
pub mod sigscheme;
pub mod sshagent;
pub mod wsproto;
pub mod pinned;

#[cfg(test)]
mod fuzzkit;
