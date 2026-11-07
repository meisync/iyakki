//! The small crate behind the desktop connector (docs/development/CONNECTOR-APP-PLAN.md, section 1): the client, the WebSocket framing,
//! the picture decoder and the certificate reader. The agent re-exports these; `iyakki-connect` uses them without the agent.

pub mod certinfo;
pub mod cloudclient;
pub mod pixelcodec;
pub mod rawlink;
pub mod sigscheme;
pub mod wsproto;
