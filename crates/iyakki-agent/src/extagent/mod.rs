//! External coding agents (Claude Code, GitHub Copilot, a custom command) that run next to the built-in models.
//! IYAKKI only drives command-line programs the person installed and signed into; it never handles a vendor password.
//! See `docs/EXTERNAL-AGENTS.md` for the verified command-line interfaces and what could not be verified.
//!
//! * [`ExternalAgent`]: what one agent is (how to detect it, how to start it, how to read its output).
//! * [`run`]: the run thread: starts the CLI, turns its event stream into the chat's `RunEvent`s, kills it on Stop.
//! * [`bridge`]: the policy bridge: IYAKKI's tools and approvals offered to the agent over a local MCP socket.
//! * [`proc`]: the one place a CLI is started and killed.

pub use zeroize::Zeroizing;

pub mod bridge;
mod facet;
pub use iyakki_shared::extagent::{claude_code, copilot, generic};
pub mod guidance;
pub mod login;
pub mod relay;
pub mod proc;
pub mod run;
#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod autopilot_tests;
#[cfg(test)]
mod auth_tests;

pub use iyakki_shared::extagent::*;
