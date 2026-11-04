//! The IYAKKI firewall. By default every new incoming connection is blocked; the person allows addresses, ranges, networks,
//! host names, ports and one-click presets. Design, defaults and recovery: `docs/FIREWALL.md`.
//!
//! * [`model`] and [`validate`]: the settings and the checks on what was typed.
//! * [`ruleset`]: settings -> typed rules (pure), and a rule evaluator for tests.
//! * [`nl`]: our own nf_tables writer (netlink), no other program involved.
//! * [`engine`]: apply, 60 s confirmation, revert, DNS refresh, expiry, status; [`service`]: the real one, process wide.
//! * [`blocked`]: the last 50 blocked attempts.

pub mod backend;
pub mod blocked;
pub mod devrules;
pub mod engine;
pub mod gaterule;
pub mod model;
pub mod namerule;
pub mod nl;
pub mod ruleset;
pub mod service;
pub mod validate;
pub mod webrule;

#[cfg(test)]
mod devrules_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod upgrade_tests;

pub use engine::{Engine, Status};
pub use model::{Config, Kind, Ping, Preset, Proto, Rule};
